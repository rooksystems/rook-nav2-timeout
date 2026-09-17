# Run the example

The experiment builds two versions of Nav2's `BtActionNode`, captures a set of scripted cases, and checks the old and fixed behavior. You can [read a completed run](result.md) before installing anything.

## Before you start

Install [Git](https://git-scm.com/downloads) and [Docker Engine](https://docs.docker.com/engine/install/) or [Docker Desktop](https://docs.docker.com/get-started/get-docker/). Start Docker and check that `docker info` succeeds.

The tested host is Linux x86_64. The container provides Ubuntu, ROS 2 Jazzy, the C++ dependencies, Python, and Rust. Docker Desktop on Apple silicon can emulate the amd64 image, but this host configuration has not been verified. Allow several gigabytes of free disk space and network access for downloads. No robot or existing ROS installation is needed.

In [the first CI run](https://github.com/rooksystems/rook-nav2-timeout/actions/runs/35153353742), setup and all checks took about 12 minutes; replay took another five. These are observations from that runner, not a timing guarantee for your machine.

## Build and capture

```sh
git clone https://github.com/rooksystems/rook-nav2-timeout.git
cd rook-nav2-timeout
./run.sh
```

You will see package installation, compilation, CTest, and JSON reports before the final summary. Downloads and compilation account for much of the first run. The script exits on a failed setup or check; keep that error output if you need help.

The final summary should include the following lines.

```text
timeout / old: FAIL (rook test exit 1)
timeout / fixed: PASS (rook test exit 0)
All 28 candidate checks matched their expected results.
```

The old candidate is expected to fail its cancellation check. The outer `run.sh` succeeds only when the complete matrix returns the expected statuses. Those include passing checks, deliberate failures, and cases where the evidence cannot settle the question. [The worked result](result.md) explains the distinction.

## Find your results

Everything produced by the container stays in ignored build directories. Start in `native/build/rook_nav2_experiment/`.

- `evidence/summary.txt` contains the final text summary. `evidence/packages.txt` and `evidence/compiler.txt` identify the installed packages and compiler.
- `integrated/timeout-old.json` and `integrated/timeout-fixed.json` contain the two timeout reports. `integrated/matrix.json` collects all candidate results.
- `nav2-replay.tar.gz`, `nav2-replay.sha256`, and `replay-offline.sh` preserve the executable replay bundle.

Keep the complete directory if you want to retain this run. Linux Docker may create files owned by root. The host Rust gate writes to `target/`; the container's Rust build writes to `native/build/rust/`.

## Replay the saved run

```sh
./replay.sh
```

This checks the archive's SHA-256, restores its files and retained packages inside a new container, then repeats all expected baseline and candidate statuses. Docker networking is disabled. No compilation or new capture occurs. Exit 0 means every status matched its expectation.

The digest-pinned base image must still be in your local Docker cache. The replay script uses `--pull never`, so a missing image causes a refusal rather than a download. The initial `run.sh` pulls that image.

## Run another build

`run.sh` refuses to replace an existing `integrated/` directory, including a partial capture from an interrupted run. Keep that directory for inspection and use a fresh checkout for another capture. Replaying an existing bundle uses its original executables; it does not test source edits made afterward.

To build again without affecting a saved run, clone into a different directory.

```sh
git clone https://github.com/rooksystems/rook-nav2-timeout.git rook-nav2-timeout-next
cd rook-nav2-timeout-next
./run.sh
```

## Troubleshooting

### Docker permission denied

On Linux, your account may lack access to the Docker socket. If you administer the machine and use Docker through sudo, run `sudo ./run.sh` and `sudo ./replay.sh`. Otherwise follow [Docker's Linux setup instructions](https://docs.docker.com/engine/install/linux-postinstall/). On Docker Desktop, check that the application is running.

### Download or package installation failed

Keep the first error and the URL or package named in it. ROS packages come from a dated, signed snapshot; Ubuntu packages come from their configured repositories. Upstream source downloads are checked against recorded hashes. An unavailable package or hash mismatch stops setup. [The workspace notes](../native/README.md) identify the package sources.

### Compiler killed or disk full

Check free disk space and Docker's memory allocation. A killed compiler may indicate a memory limit; inspect the surrounding error output before retrying. Apple silicon emulation can also make compilation slower.

### A check returned an unexpected result

Open [an issue](https://github.com/rooksystems/rook-nav2-timeout/issues) with your repository revision, host OS and architecture, command, and relevant error or report. The bundled cases are synthetic. Check any other material you attach for private information.
