#!/usr/bin/env python3
"""P1_lifecycle reconstruction policy for blockade recordings.

The participant reuses one reservation id and publishes set and cancel on
different topics, so a recorder's arrival order can show set(k) before the
cancel(k) that preceded it at the moderator; the moderator then rejects the
set as modularly stale and ignores the whole loop iteration. Policy: a cancel
with no ready/reached of the same participant between it and the preceding
set is moved to just before that set. This is Reconstructed-grade inference;
the exact processing order is only knowable with a recorder tap at the node.

Usage: reorder_lifecycle.py <in_dir> <out_dir>   (reads/writes deliveries.bin)
"""
import struct, sys, os, shutil

def reorder_messages(msgs):
    moved = 0
    moved_cancels = set()
    changed = True
    while changed:
        changed = False
        last_set_idx = {}
        blocked = set()
        for idx, (t, ch, pl) in enumerate(msgs):
            p = struct.unpack_from('<Q', pl, 0)[0]
            if ch == 10:
                last_set_idx[p] = idx
                blocked.discard(p)
            elif ch in (11, 12):
                blocked.add(p)
            elif (ch == 14 and p in last_set_idx and p not in blocked
                  and id(msgs[idx]) not in moved_cancels):
                moved_cancels.add(id(msgs[idx]))
                msgs.insert(last_set_idx[p], msgs.pop(idx))
                moved += 1
                changed = True
                break
    return moved

def main(indir, outdir):
    data = open(indir + '/deliveries.bin', 'rb').read()
    msgs = []
    i = 0
    while i < len(data):
        t, ch, ln = struct.unpack_from('<QII', data, i)
        i += 16
        msgs.append([t, ch, data[i:i+ln]])
        i += ln
    moved = reorder_messages(msgs)
    out = bytearray()
    prev = 0
    for t, ch, pl in msgs:
        t = max(t, prev + 1)
        prev = t
        out += struct.pack('<QII', t, ch, len(pl)) + pl
    os.makedirs(outdir, exist_ok=True)
    open(outdir + '/deliveries.bin', 'wb').write(bytes(out))
    shutil.copy(indir + '/heartbeats_recorded.bin', outdir)
    print(f'moved {moved} cancels before their set')

def self_test():
    participant = struct.pack('<Q', 7)
    msgs = [[1, 10, participant], [2, 10, participant], [3, 14, participant]]
    assert reorder_messages(msgs) == 1
    assert [ch for _, ch, _ in msgs] == [10, 14, 10]
    print('reorder_lifecycle self-test passed')

if __name__ == '__main__':
    if sys.argv[1:] == ['--self-test']:
        self_test()
    else:
        main(sys.argv[1], sys.argv[2])
