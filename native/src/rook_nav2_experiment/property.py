#!/usr/bin/env python3
"""Bounded properties for the pinned Nav2 tick and clock boundary."""
import json
import struct
import sys

TIMEOUT = 'nav2-goal-response-timeout-cancel@1'
TIMELY = 'nav2-timely-ack-no-cancel@1'
OVERLAP = 'nav2-overlap-results@1'
CANCEL_ACK = 'nav2-cancel-ack@1'
TERMINATION = 'nav2-goal-termination@1'


def evaluate(data):
    prop = data['property']

    def result(verdict, predicate, event=None, detail='', missing=None):
        output = dict(schema='rook-property-result@1', property=prop, result=verdict,
                      predicate=predicate, detail=detail or predicate, unavailable=[])
        if event is not None:
            output['completion_ordinal' if verdict == 'pass' else 'ordinal'] = event['ordinal']
        if missing:
            output['missing'] = missing
        if verdict == 'pass' and prop == TIMEOUT:
            output['unavailable'] = ['cancel_acknowledged', 'goal_terminated']
        return output

    completion = {TIMEOUT: 'cancel_request_issued', TIMELY: 'observation_interval_elapsed',
                  OVERLAP: 'matching_result_processed', CANCEL_ACK: 'cancel_acknowledged',
                  TERMINATION: 'goal_terminated'}.get(prop)
    if (data['schema'] != 'rook-property-input@1' or completion is None
            or data['goal_response_deadline_ns'] != 20_000_000
            or data['scope'] != dict(completion=completion, observation_end='end_of_recording')):
        return result('invalid', 'property_configuration')
    events = [dict(e, body=bytes.fromhex(e['body_hex'])) for e in data['events']]
    evidence = data['evidence']
    stopped = evidence.get('refusal') or evidence['gaps'] or not evidence['end_of_recording']
    consumed = lambda e: e['origin'] == 'recorded' and e.get('consumed') is True
    emitted = lambda e: e['origin'] == 'replayed' and e['src_actor'] == 1
    ticks = []
    clocks = []
    for e in events:
        if consumed(e) and e['event_type'] == 'Message' and e['channel_id'] == 10:
            command = json.loads(e['body'][2:])
            if command['operation'] in ('tick', 'update'):
                ticks.append((e, command['time_ns']))
        if consumed(e) and e['event_type'] == 'Clock' and e['channel_id'] == 11:
            clocks.append((e, *struct.unpack('<HqQ', e['body'])))
    if not ticks:
        return result('inconclusive', 'tick_observed', missing='initial tick command')
    first = ticks[0][0]
    callback = first['recorded_ordinal']
    sends = [e for e in events if emitted(e) and e['event_type'] == 'GoalSend'
             and e['channel_id'] == 20 and e['dst_actor'] == 3 and e['flags'] & 1
             and e.get('callback') == callback]
    if not sends:
        status = next((e for e in events if emitted(e) and e['event_type'] == 'Status'
                       and e.get('callback') == callback), None)
        if status or (not stopped and not evidence['pending_at_finish']):
            return result('fail', 'goal_submitted', first)
        return result('inconclusive', 'goal_submitted', first, missing='goal submission or callback completion')
    goal = sends[0]
    goal_id = goal['body'][:16]
    if goal['body'][16:] != bytes(8):
        return result('inconclusive', 'supported_goal', goal, missing='scenario evidence for the changed Wait goal')
    cancels = [e for e in events if emitted(e) and e['event_type'] == 'CancelSend'
               and e['channel_id'] == 21 and e['dst_actor'] == 3 and e['flags'] & 1
               and e['body'][:16] in (goal_id, bytes(16))]
    # This Nav2 API emits a zero-stamp wildcard. A timestamp-limited request
    # has different server semantics and cannot satisfy this particular case.
    for cancel in cancels if prop in (TIMEOUT, TERMINATION) else []:
        if len(cancel['body']) != 24 or cancel['body'] != bytes(24):
            return result('fail', 'valid_wildcard_cancel', cancel)
    tick_ordinals = {e['recorded_ordinal'] for e, _ in ticks}
    ros = [(e, value, cb) for e, clock, value, cb in clocks if clock == 1 and cb in tick_ordinals]
    sent_clock = next(((e, value) for e, value, cb in ros
                       if cb == callback and e['ordinal'] > goal['ordinal']), None)
    if not sent_clock:
        if cancels:
            return result('fail', 'cancel_after_deadline_expiry' if prop in (TIMEOUT, TERMINATION) else 'no_cancel_in_interval', cancels[0])
        return result('inconclusive', 'send_time_observed', goal, missing='ROS read after actual goal submission')
    deadline = sent_clock[1] + 20_000_000
    expiry = next((e for e, value, _ in ros if value >= deadline), None)
    acks = [e for e in events if consumed(e) and e['event_type'] == 'GoalResponse'
            and e['body'][8:24] == goal_id and e['body'][24] == 1]
    ack = acks[0] if acks else None
    if prop in (TIMEOUT, TERMINATION):
        if cancels:
            cancel = cancels[0]
            if not expiry or cancel['ordinal'] < expiry['ordinal'] or (ack and ack['ordinal'] < expiry['ordinal']):
                return result('fail', 'cancel_after_deadline_expiry', cancel)
            if prop == TERMINATION:
                return result('inconclusive', 'goal_terminated', cancel,
                              detail='A client request cannot establish server ordering or goal termination.',
                              missing='eligible server processing order, cancellation acknowledgment and terminal result')
            return result('pass', 'cancel_request_issued', cancel,
                          detail='Goal submitted, ROS deadline observed expired, valid wildcard cancel issued; reply and termination unavailable.')
        if not expiry:
            return result('inconclusive', 'deadline_expired', missing='ROS observation at or beyond goal-response deadline')
        failure = next((e for e in events if emitted(e) and e['event_type'] == 'Status'
                        and e['body'] == b'FAILURE' and e['ordinal'] > expiry['ordinal']), None)
        if failure or (not stopped and not evidence['pending_at_finish']):
            return result('fail', 'cancel_request_issued', failure or expiry)
        return result('inconclusive', 'cancel_request_issued', expiry, missing='input needed by the pending timeout callback')
    if prop == CANCEL_ACK:
        if not cancels:
            return result('inconclusive', 'cancel_request_issued', missing='targeted halt cancellation')
        cancel = cancels[0]
        if cancel['body'] != goal_id + bytes(8):
            return result('fail', 'targeted_halt_cancel', cancel)
        reply = next((e for e in events if consumed(e) and e['event_type'] == 'CancelResponse'
                      and len(e['body']) == 27 and e['body'][8:11] == b'\0\1\0' and e['body'][11:] == goal_id), None)
        processed = any(reply and name == 'future_completed=cancel_response' and ordinal == reply['ordinal']
                        for ordinal, name in data['progress'])
        if not reply or not processed:
            return result('inconclusive', 'cancel_acknowledged', missing='eligible processed cancellation reply')
        output = result('pass', 'cancel_acknowledged', reply)
        output['unavailable'] = ['goal_terminated']
        return output
    if prop == OVERLAP:
        if cancels:
            return result('fail', 'no_cancel_in_interval', cancels[0])
        all_goals = [e for e in events if emitted(e) and e['event_type'] == 'GoalSend'
                     and e['channel_id'] == 20 and e['dst_actor'] == 3 and e['flags'] & 1]
        if len(all_goals) != 2 or all_goals[0]['body'][:16] == all_goals[1]['body'][:16]:
            return result('inconclusive', 'overlapping_goals', missing='two distinct goal issuances')
        second = all_goals[1]
        second_id = second['body'][:16]
        accepted_second = next((e for e in events if consumed(e) and e['event_type'] == 'GoalResponse'
                                and e['body'][8:24] == second_id), None)
        stale = [e for e in events if consumed(e) and e['event_type'] == 'Result'
                 and e['body'][8:24] == goal_id and e['ordinal'] > second['ordinal']]
        current = next((e for e in events if consumed(e) and e['event_type'] == 'Result'
                        and e['body'][8:24] == second_id and e['body'][24] == 4), None)
        still_running = next((e for e in events if emitted(e) and e['event_type'] == 'Status'
                              and e['body'] == b'RUNNING' and stale and e['ordinal'] > stale[-1]['ordinal']), None)
        success = next((e for e in events if emitted(e) and e['event_type'] == 'Status'
                        and e['body'] == b'SUCCESS' and current and e['ordinal'] > current['ordinal']), None)
        if stopped or not accepted_second or len(stale) != 2 or not still_running or not success:
            return result('inconclusive', 'matching_result_processed', missing='stale result observations and a processed matching result')
        return result('pass', 'matching_result_processed', success,
                      detail='Two goals retained distinct identities; stale results left the class running and the matching result completed it.')
    if cancels:
        return result('fail', 'no_cancel_in_interval', cancels[0])
    # Property input progress pairs carry replay event ordinals. The adapter
    # processes this case's acceptance while that input owns the executor turn.
    completed_ack = any(ack and name == 'future_completed=goal_response' and ordinal == ack['ordinal']
                        for ordinal, name in data['progress'])
    before_ack_ros = next((e for e, value, cb in reversed(ros) if ack and e['ordinal'] < ack['ordinal']
                           and cb == callback and value < deadline), None)
    before_ack_steady = [(e, value) for e, clock, value, cb in clocks if ack and clock == 2
                         and cb == callback and e['ordinal'] < ack['ordinal']]
    # The script advances its steady clock from 0 to 1 ms while the first
    # 5 ms executor wait remains pending, then supplies the acceptance. No
    # elapsed wall time is substituted for these recorded observations.
    timely_wait = len(before_ack_steady) >= 2 and 0 < before_ack_steady[-1][1] - before_ack_steady[0][1] < 5_000_000
    if not ack or not completed_ack or not before_ack_ros or not timely_wait or (expiry and expiry['ordinal'] < ack['ordinal']):
        return result('inconclusive', 'acknowledged_before_deadline', missing='processed acceptance inside the observed first executor wait')
    final_tick = next((e for e, time in ticks if time >= 30_000_000), None)
    final_status = next((e for e in events if final_tick and emitted(e) and e['event_type'] == 'Status'
                         and e.get('callback') == final_tick['recorded_ordinal'] and e['body'] == b'RUNNING'), None)
    if stopped or not final_status or evidence['pending_at_finish']:
        return result('inconclusive', 'observation_interval_elapsed', missing='completed tick at 30 ms and end of recording')
    return result('pass', 'observation_interval_elapsed', final_status,
                  detail='Goal acknowledged and processed before the deadline; completed scheduled ticks through 30 ms without cancellation.')


if __name__ == '__main__':
    data = json.load(open(sys.argv[-1]))
    try:
        output = evaluate(data)
    except (KeyError, ValueError, IndexError, struct.error) as error:
        output = dict(schema='rook-property-result@1', property=data.get('property', ''),
                      result='invalid', detail=str(error), unavailable=[])
    print(json.dumps(output))
    sys.exit({'pass': 0, 'fail': 1, 'invalid': 2, 'inconclusive': 3}[output['result']])
