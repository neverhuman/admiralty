# Security policy

Report suspected vulnerabilities privately. Do not open a public issue, pull
request, discussion, or commit that includes exploit details, credentials,
tokens, private configuration, customer data, or a reproducer that exposes a
live system.

If the [GitHub Security page](https://github.com/neverhuman/admiralty/security)
offers **Report a vulnerability**, use that private GitHub form. This policy
does not establish whether private vulnerability reporting is enabled.
If it does not, contact a maintainer through a privately agreed channel and
first ask how to share details securely. Do not put vulnerability details in a
public issue to request a contact, and do not guess or publish an email address.

Include the affected commit or release, the relevant component, impact, and a
minimal reproduction with secrets and real user data removed. Share only the
information needed to verify the report. Do not test against systems you do not
own or have permission to assess. There is no published response-time or
resolution guarantee.

## Current trust boundaries

Admiralty's native service binds to loopback by default. Remote access is
expected to use an authenticated SSH tunnel. Owner and agent bearer credentials
are distinct, scope-limited, and sent in authorization headers; the dashboard
keeps its access token in page memory. Keep credential files private and outside
Git. The convenience live dashboard has a documented public loopback read of
the existing detailed feed; native data and model configuration remain
authenticated. See [native startup](docs/operators/native-startup.md) and
[release status](docs/operators/release-status.md).

Private files, directory modes, pinned descriptors, bearer tokens, and request
grants do not isolate the service from another process running as the same OS
user or from root. Do not treat a local file or a same-user process as an
independent physical-capacity or credential authority. Startup enables scoped
operator intake and agent reads; it does not install general provider-execution
or candidate-acceptance authority. A requested provider name or stored claim is
not proof that a provider ran.

Unknown execution outcomes remain held for reconciliation. Do not retry a
possibly launched effect, clear its journal, refund its reservation, or treat
missing observations as proof of closure. Preserve the original evidence and
follow the [recovery manifest guidance](docs/operators/recovery-manifest.md).
Recovery proposals do not authenticate external fencing or reactivate sessions.

These are current implementation limits, not a security certification or a
promise that every deployment has the same configuration. Deployment owners
must secure their host, credentials, private files, network path, and backups.
