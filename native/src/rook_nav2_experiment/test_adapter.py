#!/usr/bin/env python3
import json
import struct
import unittest
from adapter import Adapter


class Transport:
    def __init__(self):
        self.calls = []

    def call(self, operation, **args):
        self.calls.append((operation, args))
        return dict(waiting=3, future='goal_response', effects=[], evidence=[])


class AdapterTests(unittest.TestCase):
    def setUp(self):
        self.lines = []
        self.adapter = Adapter('unused', self.lines.append)
        self.adapter.component = Transport()
        self.adapter.state_seen = True
        self.adapter.callback = 7
        self.adapter.observation = dict(waiting=1, future='goal_response')
        self.adapter.goals = [bytes([71]) + bytes(15), bytes([19]) + bytes(15)]

    def step(self, event, body, request=None):
        actor, channel = (4, 10) if event == 'Message' else (2, 11) if event == 'Clock' else (3, 12)
        self.adapter.step(dict(input=dict(ordinal=10, src_actor=actor, dst_actor=1, channel_id=channel, event_type=event, body_hex=body.hex()), request=request))
        return self.lines[-1]

    def test_finish_before_start_has_a_protocol_refusal(self):
        adapter = Adapter('unused', self.lines.append)
        with self.assertRaisesRegex(ValueError, 'finish before start'):
            adapter.finish('scope')

    def test_another_tick_can_start_after_the_worker_completed(self):
        self.adapter.observation = dict(waiting=4, future='goal_response')
        body = b'\x02\0' + json.dumps(dict(operation='tick', time_ns=10_000_000)).encode()
        self.assertEqual(self.step('Message', body)['status'], 'ok')
        self.assertEqual(self.adapter.component.calls, [('tick', {})])

    def test_clock_must_match_the_pending_domain_and_callback(self):
        for domain, callback in ((2, 7), (1, 8)):
            response = self.step('Clock', struct.pack('<HqQ', domain, 0, callback))
            self.assertEqual(response['status'], 'not_consumed')
            self.assertEqual(self.adapter.component.calls, [])
        self.assertEqual(self.step('Clock', struct.pack('<HqQ', 1, 0, 7))['status'], 'ok')
        self.assertEqual(self.adapter.component.calls, [('clock', dict(clock=1, value=0))])

    def test_response_binds_to_its_issuance_not_the_current_goal(self):
        request = dict(ordinal=3, src_actor=1, dst_actor=3, channel_id=20, src_seq=0)
        body = struct.pack('<Q', 3) + self.adapter.goals[0] + b'\4'
        self.assertEqual(self.step('Result', body, request)['status'], 'ok')
        self.assertEqual(self.adapter.component.calls[-1], ('result', dict(issuance=0, id=list(self.adapter.goals[0]))))
        wrong = struct.pack('<Q', 3) + self.adapter.goals[1] + b'\4'
        self.assertEqual(self.step('Result', wrong, request)['status'], 'refused')
        self.assertEqual(len(self.adapter.component.calls), 1)

    def test_a_duplicate_ack_is_refused_before_touching_the_promise(self):
        request = dict(ordinal=3, src_actor=1, dst_actor=3, channel_id=20, src_seq=0)
        body = struct.pack('<Q', 3) + self.adapter.goals[0] + b'\1' + bytes(8)
        self.assertEqual(self.step('GoalResponse', body, request)['status'], 'ok')
        self.assertEqual(self.step('GoalResponse', body, request)['status'], 'refused')
        self.assertEqual(len(self.adapter.component.calls), 1)

    def test_wildcard_cancellation_invalidates_later_goal_response(self):
        self.adapter.cancels = [bytes(16)]
        request = dict(ordinal=3, src_actor=1, dst_actor=3, channel_id=20, src_seq=0)
        body = struct.pack('<Q', 3) + self.adapter.goals[0] + b'\1' + bytes(8)
        self.assertEqual(self.step('GoalResponse', body, request)['status'], 'refused')
        self.assertEqual(self.adapter.component.calls, [])

    def test_cancel_reply_needs_known_unique_goals_and_issued_cancel(self):
        self.adapter.cancels = [bytes(16)]
        request = dict(ordinal=8, src_actor=1, dst_actor=3, channel_id=21, src_seq=0)
        body = struct.pack('<QBH', 8, 0, 2) + b''.join(self.adapter.goals)
        self.assertEqual(self.step('CancelResponse', body, request)['status'], 'ok')
        duplicate = struct.pack('<QBH', 8, 0, 2) + self.adapter.goals[0] * 2
        with self.assertRaisesRegex(ValueError, 'duplicate or unknown'):
            self.step('CancelResponse', duplicate, request)

    def test_pending_cancel_is_visible_before_another_clock_read(self):
        self.adapter.observation = dict(waiting=2, future='cancel_response')
        self.assertEqual(self.adapter.pending(), [dict(kind='clock_read', callback=7, clock_id=2),
                                                 dict(kind='future', callback=7, name='cancel_response')])


if __name__ == '__main__':
    unittest.main()
