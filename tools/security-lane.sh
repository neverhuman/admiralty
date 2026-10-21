#!/usr/bin/env bash
# Jankurai security-lane entry point (the `jankurai security run` default path).
# The lane itself is Rust: crates/admiralty-xtask/src/security.rs.
exec cargo xtask security "$@"
