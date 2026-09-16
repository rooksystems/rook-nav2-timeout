#!/usr/bin/env python3
import copy
import json
import struct
import unittest
from property import evaluate, TIMEOUT, TIMELY, TERMINATION


def sample(timely=False):
    events = []

    def event(kind, body, callback=None, time=None):
        ordinal = len(events)
        emitted = kind in ('GoalSend', 'CancelSend', 'Status')
        channel = dict(Message=10, Clock=11, GoalResponse=12, GoalSend=20, CancelSend=21, Status=22)[kind]
        e = dict(ordinal=ordinal, origin='replayed' if emitted else 'recorded',
                 src_actor=1 if emitted else 4, dst_actor=5 if kind == 'Status' else 3 if emitted else 1,
                 channel_id=channel, src_seq=0, flags=1 if emitted else 0, event_type=kind,
                 kind='Emit' if emitted else 'Deliver', body_hex=body.hex())
        if emitted:
            e['callback'] = callback
        else:
            e.update(recorded_ordinal=ordinal, consumed=True)
        events.append(e)
        return ordinal

    tick = lambda t: b'\x02\0' + json.dumps(dict(operation='tick', time_ns=t)).encode()
    event('Message', tick(0))
    event('GoalSend', b'a' * 16 + bytes(8), callback=0)
    event('Clock', struct.pack('<HqQ', 1, 0, 0))
    event('Clock', struct.pack('<HqQ', 1, 0, 0))
    if timely:
        event('Clock', struct.pack('<HqQ', 2, 0, 0))
        event('Clock', struct.pack('<HqQ', 2, 1_000_000, 0))
        event('GoalResponse', struct.pack('<Q', 1) + b'a' * 16 + b'\1' + struct.pack('<q', 1_000_000))
        event('Status', b'RUNNING', callback=0)
        cb = event('Message', tick(30_000_000))
        event('Status', b'RUNNING', callback=cb)
    else:
        cb = event('Message', tick(20_000_000))
        event('Clock', struct.pack('<HqQ', 1, 20_000_000, cb))
        event('CancelSend', bytes(24), callback=cb)
    return dict(schema='rook-property-input@1', property=TIMELY if timely else TIMEOUT,
                scenario='timely' if timely else 'timeout', goal_response_deadline_ns=20_000_000,
                scope=dict(completion='observation_interval_elapsed' if timely else 'cancel_request_issued',
                           observation_end='end_of_recording'), events=events,
                progress=[[6, 'future_completed=goal_response']] if timely else [],
                evidence=dict(end_of_recording=True, gaps=[], pending_at_finish=[]), identities={})


class PropertyTests(unittest.TestCase):
    def test_request_completes_without_a_reply(self):
        data = sample()
        data['evidence']['pending_at_finish'] = [dict(kind='future', callback=4, name='cancel_response')]
        result = evaluate(data)
        self.assertEqual(result['result'], 'pass')
        self.assertEqual(result['unavailable'], ['cancel_acknowledged', 'goal_terminated'])

    def test_frozen_or_unconsumed_time_cannot_prove_expiry(self):
        for consumed in (True, False):
            data = sample()
            data['events'][5]['body_hex'] = struct.pack('<HqQ', 1, 0, 4).hex()
            data['events'][5]['consumed'] = consumed
            self.assertEqual(evaluate(data)['predicate'], 'cancel_after_deadline_expiry')
        data['events'].pop()
        self.assertEqual(evaluate(data)['result'], 'inconclusive')

    def test_unconditional_cancellation_is_rejected_in_both_cases(self):
        for timely in (True, False):
            data = sample(timely)
            cancel = copy.deepcopy(sample()['events'][-1])
            cancel['ordinal'] = 0
            data['events'].insert(0, cancel)
            self.assertEqual(evaluate(data)['result'], 'fail')

    def test_missing_goal_and_missing_evidence_are_different(self):
        data = sample()
        data['events'] = [data['events'][0]]
        self.assertEqual(evaluate(data)['result'], 'fail')
        data['evidence']['pending_at_finish'] = [dict(kind='clock_read', callback=0, clock_id=1)]
        self.assertEqual(evaluate(data)['result'], 'inconclusive')

    def test_timely_ack_must_be_processed_and_interval_must_complete(self):
        data = sample(True)
        self.assertEqual(evaluate(data)['result'], 'pass')
        for edit in ('progress', 'interval', 'gap', 'steady'):
            changed = copy.deepcopy(data)
            if edit == 'progress':
                changed['progress'] = []
            elif edit == 'interval':
                changed['events'].pop()
            elif edit == 'gap':
                changed['evidence']['gaps'] = [dict(ordinal=7, reason=1)]
            else:
                changed['events'][5]['body_hex'] = struct.pack('<HqQ', 2, 0, 0).hex()
            self.assertEqual(evaluate(changed)['result'], 'inconclusive', edit)

    def test_request_must_preserve_wildcard_timestamp_semantics(self):
        data = sample()
        data['events'][-1]['body_hex'] = (bytes(16) + struct.pack('<q', 1)).hex()
        self.assertEqual(evaluate(data)['predicate'], 'valid_wildcard_cancel')

    def test_changed_goal_needs_its_own_environment_evidence(self):
        data = sample()
        data['events'][1]['body_hex'] = (b'a' * 16 + struct.pack('<II', 2, 5)).hex()
        self.assertEqual(evaluate(data)['result'], 'inconclusive')
        self.assertEqual(evaluate(data)['predicate'], 'supported_goal')

    def test_request_evidence_never_establishes_server_order_or_termination(self):
        data = sample()
        data['property'] = TERMINATION
        data['scope']['completion'] = 'goal_terminated'
        result = evaluate(data)
        self.assertEqual(result['result'], 'inconclusive')
        self.assertEqual(result['predicate'], 'goal_terminated')
        self.assertIn('server processing order', result['missing'])


if __name__ == '__main__':
    unittest.main()
