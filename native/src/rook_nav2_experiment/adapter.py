#!/usr/bin/env python3
"""Native start/step/finish adapter around the compiled Nav2 component."""
import json
import struct
import subprocess
import sys

FRESH = 'af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262'
IDS = [[n] + [0] * 14 + [255 - n] for n in (71, 19, 203)]
ENDPOINTS = [dict(actor=n, name=name) for n, name in enumerate(
    ('session', 'component', 'clock', 'action_server', 'command', 'diagnostics'))]
CHANNELS = [dict(id=n, name=name, direction=direction, event_types=events.split(','))
            for n, name, direction, events in [
                (10, 'tick', 'input', 'Message'), (11, 'clock', 'input', 'Clock'),
                (12, 'action_response', 'input', 'GoalResponse,Feedback,Result,CancelResponse'),
                (20, 'action_goal', 'effect', 'GoalSend'),
                (21, 'action_cancel', 'effect', 'CancelSend'),
                (22, 'status', 'effect', 'Status')]]


class Component:
    def __init__(self, binary):
        self.process = subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        text=True, bufsize=1)

    def call(self, operation, **values):
        self.process.stdin.write(json.dumps(dict(operation=operation, **values)) + '\n')
        self.process.stdin.flush()
        line = self.process.stdout.readline()
        if not line:
            raise ValueError('component refused input or exited')
        return json.loads(line)

    def close(self):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        self.process.stdout.close()


class Adapter:
    def __init__(self, binary, emit):
        self.binary, self.emit = binary, emit
        self.component = None
        self.callback = 0
        self.observation = dict(waiting=0, future='')
        self.sequences = {20: 0, 21: 0, 22: 0}
        self.goals = []
        self.cancels = []
        self.acknowledged = set()
        self.cancel_replies = set()
        self.last_tick = -1
        self.last_clocks = {}
        self.state_seen = False

    def pending(self):
        waiting = self.observation['waiting']
        pending = []
        if waiting in (1, 2):
            pending.append(dict(kind='clock_read', callback=self.callback, clock_id=waiting))
        if waiting in (2, 3) and self.observation['future']:
            pending.append(dict(kind='future', callback=self.callback, name=self.observation['future']))
        return pending

    def observe(self, value, ordinal):
        self.observation = value
        for effect in value['effects']:
            name, goal_id = effect['name'], bytes(effect['id'])
            channel = 20 if name == 'goal' else 21
            body = goal_id + bytes(effect['payload'])
            if name == 'goal':
                self.goals.append(goal_id)
            else:
                self.cancels.append(goal_id)
            self.effect(ordinal, channel, 'GoalSend' if name == 'goal' else 'CancelSend', body, 1)
        for evidence in value['evidence']:
            if evidence.startswith('status='):
                self.effect(ordinal, 22, 'Status', evidence[7:].encode(), 0)
            else:
                self.emit(dict(type='progress', step=ordinal, callback=self.callback, name=evidence))

    def effect(self, ordinal, channel, event_type, body, flags):
        self.emit(dict(type='effect', step=ordinal, callback=self.callback, effect=dict(
            src_actor=1, dst_actor=5 if channel == 22 else 3, channel_id=channel,
            src_seq=self.sequences[channel], event_type=event_type, schema=1,
            flags=flags, body_hex=body.hex())))
        self.sequences[channel] += 1

    def start(self, request):
        if self.component is not None or request['protocol'] != 1 or request['corpus'] != 'nav2-action@1':
            raise ValueError('unsupported or repeated start')
        if request['state'] != dict(method='fresh', blake3=FRESH):
            raise ValueError('unsupported starting state')
        self.component = Component(self.binary)
        value = self.component.call('start', ids=IDS)
        self.observation = value
        self.emit(dict(type='started', protocol=1, identity=dict(component='nav2-bt-action',
            variant=value['variant'], adapter='rook-nav2-adapter', protocol=1),
            endpoints=ENDPOINTS, channels=CHANNELS, state=request['state']))

    def step(self, request):
        if self.component is None:
            raise ValueError('step before start')
        frame = request['input']
        ordinal = frame['ordinal']
        body = bytes.fromhex(frame['body_hex'])
        event = frame['event_type']
        status, reason = 'ok', None
        operation = None
        args = {}
        route = {'StartingState': (0, 1, 0), 'Message': (4, 1, 10), 'Clock': (2, 1, 11),
                 'GoalResponse': (3, 1, 12), 'Feedback': (3, 1, 12),
                 'Result': (3, 1, 12), 'CancelResponse': (3, 1, 12)}.get(event)
        if route is not None and tuple(frame[k] for k in ('src_actor', 'dst_actor', 'channel_id')) != route:
            self.emit(dict(type='stepped', ordinal=ordinal, status='refused',
                           reason='input endpoint or channel differs from the declared dependency', pending=self.pending()))
            return
        if event == 'StartingState':
            if self.state_seen or body != struct.pack('<H', 1) + bytes.fromhex(FRESH):
                raise ValueError('invalid starting state marker')
            self.state_seen = True
        elif not self.state_seen:
            raise ValueError('input before starting state marker')
        elif event == 'Message':
            if body[:2] != b'\x02\0':
                raise ValueError('unsupported command serialization')
            command = json.loads(body[2:])
            operation = command['operation']
            if operation not in ('tick', 'update', 'halt', 'wake'):
                raise ValueError('unsupported command')
            if operation == 'wake':
                # A recorded wake lets the private executor poll; it supplies no
                # clock value and cannot resolve an outstanding promise.
                if self.observation['waiting'] != 3:
                    status, reason, operation = 'not_consumed', 'no pending executor wait', None
            elif self.observation['waiting'] not in (0, 4):
                status, reason, operation = 'refused', 'prior callback is still pending', None
            else:
                timestamp = command['time_ns']
                if type(timestamp) is not int or timestamp < 0 or timestamp < self.last_tick:
                    raise ValueError('tick schedule went backwards')
                self.last_tick = timestamp
                self.callback = ordinal
        elif event == 'Clock':
            clock, value, callback = struct.unpack('<HqQ', body)
            if clock not in (1, 2) or value < 0 or value > 2**63 - 100_000_000 or value < self.last_clocks.get(clock, 0):
                raise ValueError('unsupported clock observation')
            if self.observation['waiting'] != clock or callback != self.callback:
                status, reason = 'not_consumed', 'clock does not answer the pending read'
            else:
                self.last_clocks[clock] = value
                operation, args = 'clock', dict(clock=clock, value=value)
        elif event in ('GoalResponse', 'Feedback', 'Result', 'CancelResponse'):
            ref = request.get('request')
            expected_channel = 21 if event == 'CancelResponse' else 20
            if not ref or ref['src_actor'] != 1 or ref['dst_actor'] != 3 or ref['channel_id'] != expected_channel:
                raise ValueError('response lacks an eligible originating request')
            if struct.unpack('<Q', body[:8])[0] != ref['ordinal']:
                raise ValueError('response request ordinal differs')
            sequence = ref['src_seq']
            if event == 'CancelResponse':
                count = struct.unpack('<H', body[9:11])[0]
                if len(body) != 11 + 16 * count or sequence >= len(self.cancels):
                    raise ValueError('invalid cancel response')
                ids = [body[n:n+16] for n in range(11, len(body), 16)]
                if len(set(ids)) != len(ids) or any(i not in self.goals for i in ids):
                    raise ValueError('duplicate or unknown cancel result ID')
                if sequence in self.cancel_replies or body[8] > 3:
                    status, reason = 'refused', 'duplicate or unsupported cancellation reply'
                else:
                    operation, args = 'cancel_reply', dict(issuance=sequence, return_code=body[8], ids=[list(i) for i in ids])
                    self.cancel_replies.add(sequence)
            else:
                goal_id = body[8:24]
                if sequence >= len(self.goals) or goal_id != self.goals[sequence]:
                    status, reason = 'refused', 'wrong or stale goal identity'
                elif any(c == bytes(16) or c == goal_id for c in self.cancels):
                    status, reason = 'refused', 'cancellation invalidated the recorded action response'
                elif event == 'GoalResponse':
                    if len(body) != 33 or body[24] != 1 or sequence in self.acknowledged:
                        status, reason = 'refused', 'only one accepted response per goal is supported'
                    else:
                        operation, args = 'ack', dict(issuance=sequence, id=list(goal_id))
                        self.acknowledged.add(sequence)
                elif event == 'Feedback':
                    if len(body) != 24:
                        raise ValueError('only empty Wait feedback is supported')
                    operation, args = 'feedback', dict(issuance=sequence, id=list(goal_id))
                else:
                    if len(body) != 25 or body[24] != 4:
                        status, reason = 'refused', 'only default successful Wait results are supported'
                    else:
                        operation, args = 'result', dict(issuance=sequence, id=list(goal_id))
        else:
            status, reason = 'refused', 'unsupported dependency ' + event
        if operation:
            self.observe(self.component.call(operation, **args), ordinal)
        self.emit(dict(type='stepped', ordinal=ordinal, status=status, reason=reason, pending=self.pending()))

    def finish(self, reason):
        if self.component is None:
            raise ValueError('finish before start')
        if reason not in ('scope', 'exhausted'):
            raise ValueError('unsupported finish reason')
        pending = self.pending()
        self.component.call('finish')
        self.emit(dict(type='finished', reason=reason, pending=pending, effects=sum(self.sequences.values())))


def serve(binary):
    adapter = Adapter(binary, lambda response: print(json.dumps(response), flush=True))
    try:
        for line in sys.stdin:
            request = json.loads(line)
            if request['type'] == 'start':
                adapter.start(request)
            elif request['type'] == 'step':
                adapter.step(request)
            elif request['type'] == 'finish':
                adapter.finish(request['reason'])
                return 0
            else:
                raise ValueError('unknown protocol request')
    except (ValueError, KeyError, IndexError, TypeError, struct.error) as error:
        print(json.dumps(dict(type='refused', reason=str(error))), flush=True)
        return 2
    finally:
        if adapter.component:
            adapter.component.close()
    return 2


if __name__ == '__main__':
    sys.exit(serve(sys.argv[1]))
