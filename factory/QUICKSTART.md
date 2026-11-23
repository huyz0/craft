# Craft Factory pilot kit

Extract the archive and open PowerShell in that directory. Git must be on PATH. Rust is required only to build the launcher from source.

```powershell
.\craft-factory.exe --version
.\craft-factory.exe --state pilot.db doctor
.\craft-factory.exe smoke --destination (Join-Path (Get-Location) 'offline-smoke')
```

The smoke test uses a fresh directory and synthetic data. Expect `passed: true` and `state: closed`. Use a new destination each time you run it.

## Verify the shared queue

With Git, Python, PostgreSQL binaries, and OpenSSL installed, run `python -B .\tools\smoke_shared_queue.py --postgres-bin $env:PGBIN`. Replace `$env:PGBIN` with the PostgreSQL `bin` path if it is unset. The test starts a temporary loopback database, applies the migrations, checks role isolation over HTTPS, and sends one Git candidate from a simulated worker to an independent verifier. Expect `"passed": true` and `"candidate_handoff": "independently verified"`.

For the shared queue, first create the PostgreSQL database and service login. Run `python .\shared\migrate_queue.py --service-role craft_queue_api` as the migration owner. Configure TLS and the operator identity. Set `CRAFT_DATABASE_URL_FILE` in the queue process environment to the path of a protected credential file readable by its service account, then run the server with `--check-config` before starting it. [Shared queue setup](shared/README.md#run-the-https-service) · [Schedule a Windows runner](deploy/windows/README.md#run-a-shared-runner-on-windows) · [Schedule the Jira relay on Windows](deploy/windows/README.md#schedule-the-jira-relay-on-windows)

## Try the workflow

For a live pilot, follow the [workflow and recovery guide](README.md). Start with the [work order](examples/work-order.json), the [review record](examples/review-record.json), and the [verification skill](skills/verify-candidate/SKILL.md). The `schemas` directory contains the JSON Schemas for this build.
