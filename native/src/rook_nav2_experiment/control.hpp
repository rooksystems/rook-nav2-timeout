#pragma once

#include <chrono>
#include <condition_variable>
#include <cstddef>
#include <cstdint>
#include <deque>
#include <exception>
#include <functional>
#include <future>
#include <memory>
#include <mutex>
#include <optional>
#include <stdexcept>
#include <string>
#include <thread>
#include <utility>
#include <vector>

#include "action_msgs/msg/goal_status.hpp"
#include "action_msgs/srv/cancel_goal.hpp"
#include "behaviortree_cpp/action_node.h"
#include "rclcpp/rclcpp.hpp"
#include "rclcpp_action/rclcpp_action.hpp"

namespace control {
using Id = rclcpp_action::GoalUUID;
enum class Wait { none, ros_clock, steady_clock, future, done };
struct Stop {};
struct Effect { std::string name; Id id; std::vector<uint8_t> payload; };

// Only the worker or its suspended caller owns component state at a time.
// The condition-variable handoff preserves the upstream stack across missing
// evidence. Ending a scope unwinds it without completing any pending promise.
struct Control {
  std::mutex mutex;
  std::condition_variable changed;
  std::thread worker;
  Wait waiting = Wait::none;
  bool stopping = false;
  int64_t supplied_time = 0;
  int64_t deadline = 0;
  std::optional<int64_t> last_ros;
  std::optional<int64_t> last_steady;
  std::string pending;
  std::exception_ptr error;
  std::vector<Effect> effects;
  std::vector<std::string> evidence;
  std::deque<std::function<void()>> callbacks;
  std::deque<Id> ids;
  size_t groups = 0;

  ~Control() { stop(); }

  void stop() {
    {
      std::lock_guard lock(mutex);
      stopping = true;
      changed.notify_all();
    }
    if (worker.joinable()) { worker.join(); }
  }

  void pause(Wait reason) {
    std::unique_lock lock(mutex);
    waiting = reason;
    changed.notify_all();
    changed.wait(lock, [&] { return waiting == Wait::none || stopping; });
    if (stopping) { throw Stop{}; }
  }

  void await() {
    std::unique_lock lock(mutex);
    changed.wait(lock, [&] { return waiting != Wait::none; });
    if (error) { std::rethrow_exception(error); }
  }

  void launch(std::function<void()> operation) {
    if (worker.joinable()) {
      if (waiting != Wait::done) { throw std::logic_error("operation still pending"); }
      worker.join();
    }
    error = nullptr;
    waiting = Wait::none;
    worker = std::thread([this, operation = std::move(operation)] {
      try { operation(); }
      catch (const Stop &) {}
      catch (...) { error = std::current_exception(); }
      std::lock_guard lock(mutex);
      waiting = Wait::done;
      changed.notify_all();
    });
    await();
  }

  void resume() {
    {
      std::lock_guard lock(mutex);
      if (waiting == Wait::done || waiting == Wait::none) {
        throw std::logic_error("no suspended operation");
      }
      waiting = Wait::none;
      changed.notify_all();
    }
    await();
  }

  int64_t clock(Wait kind) {
    pause(kind);
    auto & previous = kind == Wait::ros_clock ? last_ros : last_steady;
    if (supplied_time < 0 || (previous && supplied_time < *previous)) {
      throw std::invalid_argument("clock observation outside monotonic scenario");
    }
    previous = supplied_time;
    evidence.push_back(std::string(kind == Wait::ros_clock ? "ros=" : "steady=") + std::to_string(supplied_time));
    return supplied_time;
  }

  void dispatch() {
    while (!callbacks.empty()) {
      auto callback = std::move(callbacks.front());
      callbacks.pop_front();
      callback();
    }
  }

  void dispatch_one() {
    if (!callbacks.empty()) {
      auto callback = std::move(callbacks.front());
      callbacks.pop_front();
      callback();
    }
  }
};

// A scoped construction binding reaches the class-owned executor before the
// constructor has read its blackboard. It is never used as an event scheduler.
inline thread_local Control * constructing = nullptr;

class Executor {
  Control & control_ = *constructing;
public:
  void add_callback_group(rclcpp::CallbackGroup::SharedPtr group,
    rclcpp::node_interfaces::NodeBaseInterface::SharedPtr) {
    if (group->automatically_add_to_executor_with_node()) {
      throw std::logic_error("callback group escaped private executor");
    }
    ++control_.groups;
  }

  void spin_some() { control_.dispatch(); }

  // The executor's timed future wait is a dependency seam. Readiness still
  // comes from a real std::promise; only supplied steady-clock observations
  // can expire its wait. ROS time never answers this clock.
  template<class T, class Rep, class Period>
  rclcpp::FutureReturnCode spin_until_future_complete(
    std::shared_future<T> & future, std::chrono::duration<Rep, Period> timeout) {
    // Label the result future separately when halt moves on from the cancel
    // response. This observes the future's API type, not an action decision.
    if constexpr (requires(T value) { value.goal_id; value.code; }) {
      control_.pending = "result";
    }
    const auto ready = [&] {
      return future.wait_for(std::chrono::nanoseconds(0)) == std::future_status::ready;
    };
    if (ready()) {
      control_.evidence.push_back("future_completed=" + control_.pending);
      return rclcpp::FutureReturnCode::SUCCESS;
    }
    if (timeout <= timeout.zero()) { throw std::invalid_argument("only positive waits supported"); }
    const auto start = control_.clock(Wait::steady_clock);
    const auto deadline = start + std::chrono::duration_cast<std::chrono::nanoseconds>(timeout).count();
    control_.evidence.push_back("wait_deadline=" + std::to_string(deadline));
    for (;;) {
      control_.deadline = deadline;
      if (control_.callbacks.empty()) { control_.pause(Wait::future); }
      control_.dispatch_one();
      if (ready()) {
        control_.evidence.push_back("future_completed=" + control_.pending);
        return rclcpp::FutureReturnCode::SUCCESS;
      }
      if (control_.clock(Wait::steady_clock) >= deadline) {
        control_.evidence.push_back("wait_expired=" + control_.pending);
        return rclcpp::FutureReturnCode::TIMEOUT;
      }
    }
  }
};

template<class Action> struct GoalHandle {
  using SharedPtr = std::shared_ptr<GoalHandle>;
  using WrappedResult = typename rclcpp_action::ClientGoalHandle<Action>::WrappedResult;
  Id id;
  int8_t status = action_msgs::msg::GoalStatus::STATUS_ACCEPTED;
  const Id & get_goal_id() const { return id; }
  int8_t get_status() const { return status; }
};

// This is the action-client API boundary, outside the replayed class. Each
// issuance retains its own promise and callbacks even after a replacement.
// It implements no Nav2 timeout, cancellation policy, or result filtering.
template<class Action> struct Client {
  using SharedPtr = std::shared_ptr<Client>;
  using Handle = GoalHandle<Action>;
  using Cancel = action_msgs::srv::CancelGoal::Response::SharedPtr;
  struct SendGoalOptions {
    std::function<void(const typename Handle::WrappedResult &)> result_callback;
    std::function<void(typename Handle::SharedPtr, std::shared_ptr<const typename Action::Feedback>)> feedback_callback;
  };
  struct Issuance {
    typename Handle::SharedPtr handle;
    std::promise<typename Handle::SharedPtr> promise;
    SendGoalOptions options;
    bool acknowledged = false;
  };
  Control & control;
  std::vector<std::shared_ptr<Issuance>> goals;
  std::vector<std::shared_ptr<std::promise<Cancel>>> cancels;
  std::promise<typename Handle::WrappedResult> result;

  explicit Client(Control & value) : control(value) {}
  bool wait_for_action_server(std::chrono::milliseconds) { return true; }

  auto async_send_goal(const typename Action::Goal & value, SendGoalOptions options) {
    if (control.ids.empty()) { throw std::logic_error("missing supplied goal ID"); }
    auto goal = std::make_shared<Issuance>();
    goal->handle = std::make_shared<Handle>();
    goal->handle->id = control.ids.front();
    control.ids.pop_front();
    goal->options = std::move(options);
    goals.push_back(goal);
    // Capture the actual Wait goal passed to the client API. Replay must not
    // replace a candidate's changed goal fields with a fixture's default bytes.
    std::vector<uint8_t> payload;
    for (uint32_t field : {static_cast<uint32_t>(value.time.sec), value.time.nanosec}) {
      for (unsigned shift = 0; shift < 32; shift += 8) {
        payload.push_back(static_cast<uint8_t>(field >> shift));
      }
    }
    control.effects.push_back({"goal", goal->handle->id, std::move(payload)});
    control.pending = "goal_response";
    return goal->promise.get_future().share();
  }

  void acknowledge(size_t issuance, const Id & id) {
    auto goal = goals.at(issuance);
    if (goal->handle->id != id || goal->acknowledged) {
      throw std::invalid_argument("goal response does not bind to an unresolved issuance");
    }
    goal->acknowledged = true;
    control.callbacks.push_back([this, goal] {
      control.evidence.push_back("ack=" + std::to_string(goal->handle->id[0]));
      goal->promise.set_value(goal->handle);
    });
  }

  void deliver_result(size_t issuance, const Id & id) {
    auto goal = goals.at(issuance);
    control.callbacks.push_back([goal, id] {
      typename Handle::WrappedResult value{};
      value.goal_id = id;
      value.code = rclcpp_action::ResultCode::SUCCEEDED;
      value.result = std::make_shared<typename Action::Result>();
      goal->options.result_callback(value);
    });
  }

  void deliver_feedback(size_t issuance) {
    auto goal = goals.at(issuance);
    control.callbacks.push_back([goal] {
      goal->options.feedback_callback(goal->handle, std::make_shared<typename Action::Feedback>());
    });
  }

  auto cancel(Id id) {
    auto promise = std::make_shared<std::promise<Cancel>>();
    cancels.push_back(promise);
    control.effects.push_back({"cancel", id, std::vector<uint8_t>(8, 0)});
    control.pending = "cancel_response";
    return promise->get_future().share();
  }
  auto async_cancel_all_goals() { return cancel(Id{}); }
  auto async_cancel_goal(typename Handle::SharedPtr goal) { return cancel(goal->id); }
  auto async_get_result(typename Handle::SharedPtr) { return result.get_future().share(); }
};

class Node : public rclcpp::Node {
  Control & control_;
public:
  using SharedPtr = std::shared_ptr<Node>;
  explicit Node(Control & control) : rclcpp::Node("nav2_compile_control"), control_(control) {}
  rclcpp::Time now() { return rclcpp::Time(control_.clock(Wait::ros_clock), RCL_ROS_TIME); }
  template<class Action>
  auto create_action_client(const std::string &, rclcpp::CallbackGroup::SharedPtr) {
    return std::make_shared<Client<Action>>(control_);
  }
};
}  // namespace control

// Same port-first, blackboard-second lookup as the upstream bt_utils helper.
// Unrelated geometry/JSON serializers pull newer ROS message types into Jazzy.
namespace BT {
template<class T>
bool getInputPortOrBlackboard(const TreeNode & node, const Blackboard & board,
  const std::string & name, T & value) {
  if (node.getInput<T>(name, value)) { return true; }
  return board.get<T>(name, value);
}
template<> inline std::chrono::milliseconds convertFromString(StringView value) {
  return std::chrono::milliseconds(std::stoul(std::string(value)));
}
}
#define getInputOrBlackboard(name, value) \
  getInputPortOrBlackboard(*this, *(this->config().blackboard), name, value)
