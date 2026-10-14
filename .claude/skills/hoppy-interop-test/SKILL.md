---
name: hoppy-interop-test
description: Use when two Hoppy implementations disagree (Rust CLI, Android Kotlin core, Python reference), when debugging discovery on a real network, or when adding a cross-language test.
---

# Interop testing and LAN debugging

## Automated

`python3 tools/dev.py interop` → `tools/interop.sh`:
1. Rust → Python, 2. Python → Rust, 3. discovery Python sees Rust;
with `HOPPY_KT_CP` (set automatically when Gradle exists): 4. Kotlin → Rust,
5. Rust → Kotlin, 6. discovery Kotlin sees Rust.
The script sets `LC_ALL=C.UTF-8`: the JVM needs it for non-ASCII file names.

## Bisecting a disagreement

1. Reproduce with the Python reference in the middle — it is the most readable
   implementation: `python3 tools/hoppy_ref.py receive /tmp/in` then send from the suspect.
2. Compare against `docs/test-vectors.json`; if the spec is ambiguous, fix the spec first.
3. Add the failing case to the vectors so all three test suites catch it.

## Real network

```
python3 tools/hoppy_ref.py peers --wait 5              # multicast + broadcast
python3 tools/hoppy_ref.py peers --seed 192.168.1.20   # unicast, bypasses multicast
hoppy peers --seed 192.168.1.20                        # same from the Rust CLI
```
If unicast works but multicast does not: router/AP isolation or multicast filtering.
On Android, discovery needs the app's `MulticastLock` (held by `Node`).
