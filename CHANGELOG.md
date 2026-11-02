# Changelog

All notable changes to this crate are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-11-02

### Added

- Adopt as an idle dog that answers shep sandbox with its verbs
- Write a commented crabpot settings file with shep sandbox init
- Check what a sandboxed sheep needs on this host
- Fill the settings from a running sheep with --from
- Check every sheep in the flock whose sandbox key is set
- Remove the v1 init and doctor verbs and the sandbox.json path **(BREAKING)**
- Serve crabpot's per-sheep settings schema and validate a sheep's table
- Launch <sheep> -- <cmd> runs the command inside a crabpot sandbox
- Declare launch in the manifest
- A running sheep follows its sandbox settings and applies network list edits live

### Changed

- Move the from_a_sheep tests to their own file

### Fixed

- Tell `shep sandbox` apart from the supervised dog
- Remove the file when writing the skeleton fails
- Pass a sheep's settings path to crabpot as written
- Match a listener by address when sorting dials
- Read lsof when one pid has no socket, and tell a gone sheep
- Write only the network keys crabpot applies on this host
- Keep Windows out of the advertised --from path, test run's contents
- Probe the shepherd channel under settings crabpot accepts
- Leave a sheep unchecked when its cwd is unknown or its config unreadable
- Release the dog daily on a crabpot bump and merge it when CI passes
- Bound the auto-merge job with a timeout
- Fold the crabpot entry into one daily cargo entry
- Enable crabpot auto-merge with the release PAT
- Stop on the shepherd's shutdown message and adopt the integration dog under another name
- Tighten the launch tests and docs after review
- Say a removed table refuses the next start, retry an unreadable table, keep the running baseline
- Back off while the shepherd keeps refusing the table read
- Build the dog against shep 0.23.0

