#!/usr/bin/env python3
"""Capture the scripted environment directly from the old component transport.

This does not execute the replay adapter or consult property expectations. It
retains the component transcript alongside the input/effect recording. The
packer only serializes these observations; candidate runs never recapture them.
"""
import json
from pathlib import Path
import struct
import subprocess
import sys

IDS = [[n] + [0] * 14 + [255 - n] for n in (71, 19, 203)]
FRESH = 'af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262'


def capture(binary, scenario):
    process = subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    transcript, frames, captured = [], [], []
    sequences = {}
    callback = 0
    goals, cancels = [], []

    def call(operation, **values):
        request = dict(operation=operation, **values)
        process.stdin.write(json.dumps(request) + '\n')
        process.stdin.flush()
        line = process.stdout.readline()
        if not line:
            raise ValueError('old component exited during capture')
        response = json.loads(line)
        transcript.append(dict(request=request, response=response))
        return response

    def frame(actor, destination, channel, event, body, flags=0):
        sequence = sequences.get((actor, channel), 0)
        sequences[actor, channel] = sequence + 1
        value = dict(ordinal=len(frames) + 1, src_actor=actor, dst_actor=destination,
                     channel_id=channel, src_seq=sequence, event_type=event, schema=1,
                     flags=flags, body_hex=body.hex())
        frames.append(value)
        return value

    def observe(value):
        for effect in value['effects']:
            goal_id = bytes(effect['id'])
            if effect['name'] == 'goal':
                item = frame(1, 3, 20, 'GoalSend', goal_id + bytes(effect['payload']), 1)
                goals.append(item)
            else:
                item = frame(1, 3, 21, 'CancelSend', goal_id + bytes(effect['payload']), 1)
                cancels.append(item)
            captured.append(dict(item))
        for entry in value['evidence']:
            if entry.startswith('status='):
                captured.append(dict(frame(1, 5, 22, 'Status', entry[7:].encode())))

    def command(operation, time_ns=0):
        nonlocal callback
        value = frame(4, 1, 10, 'Message', b'\x02\0' + json.dumps(dict(operation=operation, time_ns=time_ns), separators=(',', ':')).encode())
        if operation != 'wake':
            callback = value['ordinal']
        result = call(operation)
        observe(result)
        return result

    def clock(kind, value):
        frame(2, 1, 11, 'Clock', struct.pack('<HqQ', kind, value, callback))
        result = call('clock', clock=kind, value=value)
        observe(result)
        return result

    def response(operation, issuance, supplied_id=None):
        goal = goals[issuance]
        goal_id = supplied_id or bytes.fromhex(goal['body_hex'])[:16]
        prefix = struct.pack('<Q', goal['ordinal']) + goal_id
        event, suffix = dict(ack=('GoalResponse', b'\1' + struct.pack('<q', 1_000_000)),
                             feedback=('Feedback', b''), result=('Result', b'\4'))[operation]
        frame(3, 1, 12, event, prefix + suffix)
        observe(call(operation, issuance=issuance, id=list(goal_id)))

    try:
        call('start', ids=IDS)
        frame(0, 1, 0, 'StartingState', struct.pack('<H', 1) + bytes.fromhex(FRESH))
        command('tick', 0)
        acknowledged = scenario in ('timely', 'overlap', 'wrong-result', 'cancel-ack')
        if scenario != 'missing-clock':
            clock(1, 0)
            clock(1, 0)
            clock(2, 0)
            command('wake')
            clock(2, 1_000_000 if acknowledged else 0 if scenario == 'frozen-clock' else 5_000_000)
        if acknowledged:
            response('ack', 0)
        if scenario == 'timely':
            command('tick', 10_000_000)
            command('tick', 30_000_000)
        elif scenario in ('overlap', 'wrong-result'):
            command('update', 10_000_000)
            clock(1, 10_000_000)
            clock(1, 10_000_000)
            clock(2, 1_000_000)
            response('result', 0)
            clock(2, 2_000_000)
            response('ack', 1)
            response('result', 0)
            response('feedback', 1)
            command('tick', 20_000_000)
            response('result', 1, bytes.fromhex(goals[0]['body_hex'])[:16] if scenario == 'wrong-result' else None)
            command('tick', 30_000_000)
        elif scenario == 'cancel-ack':
            command('halt', 10_000_000)
            clock(2, 1_000_000)
            cancel = cancels[0]
            goal_id = bytes.fromhex(goals[0]['body_hex'])[:16]
            frame(3, 1, 12, 'CancelResponse', struct.pack('<QBH', cancel['ordinal'], 0, 1) + goal_id)
            observe(call('cancel_reply', issuance=0, return_code=0, ids=[list(goal_id)]))
        elif scenario in ('timeout', 'cancel-before-goal'):
            command('tick', 10_000_000)
            clock(1, 10_000_000)
            clock(2, 5_000_000)
            command('wake')
            clock(2, 10_000_000)
            command('tick', 20_000_000)
            clock(1, 20_000_000)
        elif scenario not in ('missing-clock', 'frozen-clock'):
            raise ValueError('unknown capture scenario')
        call('finish')
        return dict(scenario=scenario, frames=frames, captured=captured, transcript=transcript)
    finally:
        process.stdin.close()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        process.stdout.close()


if __name__ == '__main__':
    Path(sys.argv[3]).write_text(json.dumps(capture(sys.argv[1], sys.argv[2]), indent=2) + '\n')
