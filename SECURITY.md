# Security Policy

## Supported Releases

Security fixes are provided for the most recent stable GitHub release. Older
releases, prereleases, development branches, and locally modified builds should
be upgraded or reproduced against the current stable release before a report is
closed.

| Version | Security fixes |
| --- | --- |
| Latest stable release | Supported |
| Older stable releases | Not supported |
| Prereleases and source snapshots | Not supported for production |

## Reporting a Vulnerability

Use the repository **Security** tab and its **Report a vulnerability** action
when private vulnerability reporting is available. Include:

- the affected release tag and deployment model;
- the preconditions and minimum reproduction;
- the security impact and affected trust boundary;
- relevant logs with credentials, private keys, operator tokens, active
  monitoring-share URLs, hostnames, and operator data removed; and
- any suggested mitigation or evidence that the issue is already exploited.

If private reporting is not available, do not disclose exploit details or
secrets in a public issue. Open a minimal issue asking the maintainers to enable
or identify a private reporting channel, then wait for that channel before
sending sensitive details. No response-time or remediation SLA is currently
promised.

Immediately rotate any credential or key that was exposed while reproducing or
reporting an issue. Never submit production database dumps, agent private keys,
operator privilege material, internal tokens, live access tokens, or active
monitoring-share URLs. Revoke an exposed shared view and create a replacement;
its bearer secret is retained server-side for authorized Copy URL recovery but
cannot be rotated in place, and a revoked or expired link cannot be reactivated.

## Security-Relevant Scope

Reports are especially useful when they demonstrate authentication or
authorization bypass, cross-VPS access, privilege-assertion bypass, command or
path injection, unsafe release/update behavior, secret disclosure, integrity
failure in agent communication or release verification, or isolation failure
between public monitoring projections, the operator console, internal API, and
gateway-control interfaces.

The default deployment is intentionally private and loopback-bound. Exposing
the console or raw TCP agent gateway changes the deployment trust boundary and
must be done with the controls described in
[Production Deployment](docs/production-deployment.md). An operator-created
monitoring share is the deliberate exception: it publishes an expiring,
token-authorized, allowlisted read-only projection through the existing HTTPS
frontend origin, not the API container itself.
