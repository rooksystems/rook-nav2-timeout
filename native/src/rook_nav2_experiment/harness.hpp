#pragma once

#include "nav2_msgs/action/wait.hpp"
#include UPSTREAM_HEADER

#include <algorithm>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <exception>
#include <iostream>
#include <memory>
#include <sstream>
#include <stdexcept>
#include <string>
#include <utility>

using namespace std::chrono_literals;
using Action = nav2_msgs::action::Wait;
using control::Wait;

void require(bool condition, const std::string & predicate) {
  if (!condition) { throw std::logic_error(predicate); }
}

control::Id id(uint8_t value) {
  control::Id result{};
  result[0] = value;
  result[15] = 255 - value;
  return result;
}

struct ActionNode : nav2_behavior_tree::BtActionNode<Action> {
  std::string mutant;
  bool update = false;
  size_t feedback_seen = 0;
  explicit ActionNode(const BT::NodeConfiguration & config, std::string mutation)
  : BtActionNode("Wait", "wait", config), mutant(std::move(mutation)) {}
  void on_tick() override {
    if (mutant == "noop") { should_send_goal_ = false; }
    if (mutant == "always-cancel") { action_client_->async_cancel_all_goals(); }
  }
  void on_wait_for_result(std::shared_ptr<const Action::Feedback> feedback) override {
    if (feedback) { ++feedback_seen; }
    if (update) { goal_updated_ = true; update = false; }
  }
  auto client() { return action_client_; }
};

struct Experiment {
  control::Control control;
  std::shared_ptr<control::Node> ros;
  std::unique_ptr<ActionNode> action;
  BT::NodeStatus returned = BT::NodeStatus::IDLE;
  int64_t steady = 0;

  explicit Experiment(const std::string & mutant = "none", std::chrono::milliseconds loop = 10ms) {
    control::constructing = &control;
    ros = std::make_shared<control::Node>(control);
    BT::NodeConfiguration config;
    config.blackboard = BT::Blackboard::create();
    config.blackboard->set("node", ros);
    config.blackboard->set("bt_loop_duration", loop);
    config.blackboard->set("server_timeout", 20ms);
    config.blackboard->set("cancel_timeout", 50ms);
    config.blackboard->set("wait_for_service_timeout", 100ms);
    config.output_ports["error_code_id"] = "{error_code_id}";
    config.output_ports["error_msg"] = "{error_msg}";
    action = std::make_unique<ActionNode>(config, mutant);
    control::constructing = nullptr;
    control.ids = {id(71), id(19), id(203)};
    require(control.groups == 1, "class_owned_executor_registered");
  }

  ~Experiment() { control.stop(); }

  // This drives only dependency observations. Tick branches and elapsed-time
  // arithmetic remain in the pinned upstream header. The iteration bound
  // detects missing progress without consulting a wall clock.
  void drive(int64_t ros_time, bool acknowledge) {
    for (size_t steps = 0; steps < 128; ++steps) {
      if (control.waiting == Wait::done) { return; }
      if (control.waiting == Wait::future && control.pending == "cancel_response") { return; }
      if (control.waiting == Wait::ros_clock) { control.supplied_time = ros_time; }
      if (control.waiting == Wait::steady_clock) { control.supplied_time = steady; }
      if (control.waiting == Wait::future) {
        if (acknowledge) {
          auto client = action->client();
          auto issuance = client->goals.size() - 1;
          client->acknowledge(issuance, client->goals.at(issuance)->handle->id);
          steady += 1000000;
        } else {
          steady = control.deadline;
        }
      }
      control.resume();
    }
    throw std::logic_error("dependency progress bound exceeded");
  }

  void tick(int64_t ros_time, bool acknowledge = false) {
    control.evidence.push_back("tick=" + std::to_string(ros_time));
    control.launch([&] {
      returned = action->executeTick();
      control.evidence.push_back("status=" + std::string(BT::toStr(returned)));
    });
    drive(ros_time, acknowledge);
  }

  size_t count(const std::string & name) const {
    return std::count_if(control.effects.begin(), control.effects.end(),
      [&](const auto & event) { return event.name == name; });
  }

  std::string effects() const {
    std::ostringstream output;
    for (const auto & effect : control.effects) {
      output << effect.name << "=";
      for (auto byte : effect.id) { output << std::hex << int(byte) << ","; }
      output << "\n";
    }
    return output.str();
  }
};
