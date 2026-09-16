// Private component transport. The public adapter owns the native wire format;
// this executable only exposes dependency observations around the upstream class.
#include "harness.hpp"
#include <nlohmann/json.hpp>

using Json = nlohmann::json;

control::Id parse_id(const Json & value) {
  auto bytes = value.get<std::vector<uint8_t>>();
  require(bytes.size() == 16, "goal ID must contain 16 bytes");
  control::Id result;
  std::copy(bytes.begin(), bytes.end(), result.begin());
  return result;
}

int main(int argc, char ** argv) {
  rclcpp::init(argc, argv);
  try {
    Experiment run(MUTATION);
    auto & c = run.control;
    size_t effects = 0;
    size_t evidence = 0;
    std::string line;
    bool started = false;
    while (std::getline(std::cin, line)) {
      const auto input = Json::parse(line);
      const auto operation = input.at("operation").get<std::string>();
      if (operation == "start") {
        require(!started, "duplicate start");
        started = true;
        c.ids.clear();
        for (const auto & value : input.at("ids")) {
          auto goal_id = parse_id(value);
          goal_id[0] += GOAL_ID_OFFSET;
          c.ids.push_back(goal_id);
        }
      } else {
        require(started, "operation before start");
        if (operation == "tick" || operation == "update" || operation == "halt") {
          require(!c.worker.joinable() || c.waiting == Wait::done, "callback still pending");
          if (operation == "update") { run.action->update = true; }
          c.launch([&, operation] {
            if (operation == "halt") {
              run.action->halt();
              c.evidence.push_back("halted");
            } else {
              run.returned = run.action->executeTick();
              c.evidence.push_back("status=" + std::string(BT::toStr(run.returned)));
            }
          });
        } else if (operation == "clock") {
          const auto kind = input.at("clock").get<int>() == 1 ? Wait::ros_clock : Wait::steady_clock;
          require(c.waiting == kind, "wrong or unrequested clock observation");
          c.supplied_time = input.at("value").get<int64_t>();
          c.resume();
        } else if (operation == "wake") {
          require(c.waiting == Wait::future, "executor is not waiting");
          c.resume();
        } else if (operation == "ack" || operation == "feedback" || operation == "result") {
          auto client = run.action->client();
          auto issuance = input.at("issuance").get<size_t>();
          auto goal_id = parse_id(input.at("id"));
          if (operation == "ack") { client->acknowledge(issuance, goal_id); }
          if (operation == "feedback") {
            require(client->goals.at(issuance)->handle->id == goal_id, "feedback ID differs");
            client->deliver_feedback(issuance);
          }
          if (operation == "result") { client->deliver_result(issuance, goal_id); }
          // Queueing is observable separately from processing. Only the private
          // executor can execute the callback, either now or on a later tick.
          if (c.waiting == Wait::future) { c.resume(); }
        } else if (operation == "cancel_reply") {
          auto client = run.action->client();
          auto promise = client->cancels.at(input.at("issuance").get<size_t>());
          auto response = std::make_shared<action_msgs::srv::CancelGoal::Response>();
          response->return_code = input.at("return_code").get<int8_t>();
          for (const auto & value : input.at("ids")) {
            action_msgs::msg::GoalInfo goal;
            goal.goal_id.uuid = parse_id(value);
            response->goals_canceling.push_back(goal);
          }
          c.callbacks.push_back([promise, response] { promise->set_value(response); });
          if (c.waiting == Wait::future) { c.resume(); }
        } else if (operation != "finish") { throw std::invalid_argument("unsupported operation"); }
      }
      Json output = {{"effects", Json::array()}, {"evidence", Json::array()},
        {"waiting", static_cast<int>(c.waiting)}, {"future", c.pending},
        {"variant", COMPONENT_VARIANT}};
      for (; effects < c.effects.size(); ++effects) {
        output["effects"].push_back({{"name", c.effects[effects].name}, {"id", c.effects[effects].id}, {"payload", c.effects[effects].payload}});
      }
      for (; evidence < c.evidence.size(); ++evidence) { output["evidence"].push_back(c.evidence[evidence]); }
      std::cout << output.dump() << std::endl;
      if (operation == "finish") { break; }
    }
    rclcpp::shutdown();
    return 0;
  } catch (const std::exception & error) {
    std::cerr << "Nav2 component refused: " << error.what() << "\n";
    rclcpp::shutdown();
    return 2;
  }
}
