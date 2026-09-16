#include "harness.hpp"

// These compile-experiment assertions are not the product property program.
// In particular they do not make baseline verification or recording-grade claims.
std::string timeout_case(const std::string & mutant, bool fixed) {
  Experiment run(mutant);
  for (int64_t time : {0LL, 10000000LL, 20000000LL}) {
    run.tick(time);
    if (run.returned == BT::NodeStatus::FAILURE || run.control.pending == "cancel_response") { break; }
  }
  std::string predicate;
  if (run.count("goal") == 0) { predicate = "goal_submitted"; }
  else if (!run.control.effects.empty() && run.control.effects.front().name == "cancel") {
    predicate = "cancel_after_deadline_expiry";
  }
  else if (run.count("cancel") == 0) { predicate = "cancel_request_issued"; }
  else {
    require(fixed, "only_fixed_issues_timeout_cancel");
    require(run.control.waiting == Wait::future, "cancel_observable_before_tick_returns");
    require(run.control.pending == "cancel_response", "cancel_response_pending");
    require(run.action->client()->cancels.size() == 1, "one_cancel_request");
    require(run.control.effects.back().id == control::Id{}, "cancel_is_wildcard");
    require(run.returned == BT::NodeStatus::RUNNING, "timeout_tick_has_not_returned");
    require(std::find(run.control.evidence.begin(), run.control.evidence.end(), "ros=20000000") != run.control.evidence.end(), "deadline_observed");
    predicate = "request_complete_reply_unavailable";
  }
  const auto expected = mutant == "noop" ? "goal_submitted" : mutant == "always-cancel" ?
    "cancel_after_deadline_expiry" : fixed ? "request_complete_reply_unavailable" : "cancel_request_issued";
  require(predicate == expected, "timeout_matrix");
  std::cout << "timeout " << VARIANT << " " << mutant << " predicate=" << predicate << "\n" << run.effects();
  for (const auto & event : run.control.evidence) { std::cout << event << "\n"; }
  return run.effects();
}

void timely_case(const std::string & mutant) {
  Experiment run(mutant);
  run.tick(0, true);
  if (mutant != "noop") {
    require(run.returned == BT::NodeStatus::RUNNING, "ack_processed_without_timeout");
    require(run.action->client()->goals.at(0)->acknowledged, "ack_issued");
    require(std::find(run.control.evidence.begin(), run.control.evidence.end(), "future_completed=goal_response") != run.control.evidence.end(), "ack_processed");
    run.tick(10000000);
    run.tick(30000000);
    require(std::find(run.control.evidence.begin(), run.control.evidence.end(), "tick=30000000") != run.control.evidence.end(), "observation_interval_reached");
  }
  const auto predicate = run.count("goal") == 0 ? "goal_submitted" : run.count("cancel") ?
    "no_cancel_in_interval" : "timely_ack_observation_complete";
  const auto expected = mutant == "noop" ? "goal_submitted" : mutant == "always-cancel" ?
    "no_cancel_in_interval" : "timely_ack_observation_complete";
  require(std::string(predicate) == expected, "timely_matrix");
  std::cout << "timely " << VARIANT << " " << mutant << " predicate=" << predicate << "\n" << run.effects();
  for (const auto & event : run.control.evidence) { std::cout << event << "\n"; }
}

void overlapping_results(bool fixed) {
  Experiment run;
  run.tick(0, true);
  auto client = run.action->client();
  bool wrong_ack_refused = false;
  try { client->acknowledge(0, id(203)); }
  catch (const std::invalid_argument &) { wrong_ack_refused = true; }
  require(wrong_ack_refused, "wrong_or_duplicate_ack_refused");
  run.action->update = true;
  run.tick(10000000);
  require(client->goals.size() == 2, "overlapping_issuances_preserved");
  require(client->goals[0]->handle->id == id(71) && client->goals[1]->handle->id == id(19), "supplied_goal_ids_preserved");
  client->deliver_result(0, id(71));
  run.tick(20000000, true);
  require(run.returned == BT::NodeStatus::RUNNING, "stale_result_during_ack_ignored");
  client->deliver_result(0, id(71));
  client->deliver_result(1, id(203));
  client->deliver_feedback(1);
  run.tick(25000000);
  require(run.returned == BT::NodeStatus::RUNNING, "wrong_and_stale_result_not_relabelled");
  client->deliver_result(1, id(19));
  run.tick(30000000);
  require(run.action->feedback_seen == 1, "feedback_processed_once");
  require(run.returned == BT::NodeStatus::SUCCESS, "matching_result_completes");
  std::cout << "overlap " << VARIANT << " predicate=issuance_and_result_identity\n";

  // Reach the separate updated-goal timeout branch inside the same tick.
  Experiment update("none", 100ms);
  update.tick(0, true);
  update.action->update = true;
  update.tick(10000000);
  require(update.count("goal") == 2, "updated_goal_submitted");
  require(update.count("cancel") == size_t(fixed), "updated_goal_timeout_cancel");
  if (fixed) {
    require(update.control.waiting == Wait::future && update.control.pending == "cancel_response", "updated_cancel_pending");
  } else { require(update.returned == BT::NodeStatus::FAILURE, "old_updated_goal_failed"); }
  std::cout << "updated-timeout " << VARIANT << " predicate=" << (fixed ? "request_complete_reply_unavailable" : "cancel_request_issued") << "\n";
}

void missing_and_frozen_clock() {
  Experiment run;
  run.control.launch([&] { run.returned = run.action->executeTick(); });
  require(run.control.waiting == Wait::ros_clock, "missing_ros_clock_stays_pending");
  require(run.count("goal") == 1, "submission_precedes_ros_read");
  for (size_t step = 0; step < 24; ++step) {
    run.control.supplied_time = 0;
    run.control.resume();
    require(run.control.waiting != Wait::done, "frozen_clock_cannot_expire_wait");
    require(run.count("cancel") == 0, "frozen_clock_cannot_pass_timeout");
  }
  require(run.returned == BT::NodeStatus::IDLE, "tick_still_pending");
  std::cout << "missing-and-frozen-clock " << VARIANT << " predicate=evidence_missing\n";
}

void halt_limitation() {
  Experiment run;
  run.tick(0);
  require(run.returned == BT::NodeStatus::RUNNING, "halt_has_pending_goal");
  run.control.launch([&] { run.action->halt(); });
  require(run.control.waiting == Wait::done, "halt_returned");
  require(run.count("goal") == 1 && run.count("cancel") == 0, "halt_before_ack_uncovered");
  std::cout << "halt-before-ack " << VARIANT << " limitation=6426_unfixed\n";
}

void ready_future_and_exception_handoff() {
  control::Control control;
  control::constructing = &control;
  control::Executor executor;
  control::constructing = nullptr;
  bool caught = false;
  try { control.launch([] { throw std::invalid_argument("unsupported input"); }); }
  catch (const std::invalid_argument &) { caught = true; }
  require(caught, "worker_exception_delivered");
  std::promise<int> promise;
  auto future = promise.get_future().share();
  promise.set_value(7);
  control.launch([&] {
    require(executor.spin_until_future_complete(future, 20ms) == rclcpp::FutureReturnCode::SUCCESS,
      "ready_future_completes");
  });
  require(control.waiting == Wait::done, "ready_future_needs_no_clock");
  std::cout << "ready-future " << VARIANT << " predicate=no_clock_or_stale_exception\n";
}

void capture_actual_goal_fields() {
  control::Control control;
  control.ids.push_back(id(71));
  control::Client<Action> client(control);
  Action::Goal goal;
  goal.time.sec = 2;
  goal.time.nanosec = 5;
  auto future = client.async_send_goal(goal, {});
  require(future.valid(), "submitted_goal_has_future");
  require(control.effects.at(0).payload == std::vector<uint8_t>({2, 0, 0, 0, 5, 0, 0, 0}),
    "actual_goal_fields_captured");
}

int main(int argc, char ** argv) {
  rclcpp::init(argc, argv);
  try {
    const bool fixed = std::string(VARIANT) == "fixed";
    const auto captured = timeout_case("none", fixed);
    for (size_t repeat = 0; repeat < 2; ++repeat) {
      require(timeout_case("none", fixed) == captured, "repeated_effect_bytes_agree");
    }
    for (const auto & mutant : {"none", "noop", "always-cancel"}) {
      timeout_case(mutant, fixed);
      timely_case(mutant);
    }
    overlapping_results(fixed);
    missing_and_frozen_clock();
    halt_limitation();
    ready_future_and_exception_handoff();
    capture_actual_goal_fields();
    rclcpp::shutdown();
    return 0;
  } catch (const std::exception & error) {
    std::cerr << "experiment failed predicate=" << error.what() << "\n";
    rclcpp::shutdown();
    return 1;
  }
}
