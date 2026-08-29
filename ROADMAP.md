# SSE Discord Bot Roadmap

Last reviewed: 2026-08-28

This document defines the path from the current implementation to a dependable first production release. It is the shared reference for what is implemented, what remains, and what "complete" means for the bot.

## Product Goal

The bot should let SSE members verify their identity and let authorized officers approve infrastructure access from Discord without making Discord the system of record. Workflow state must survive restarts, privileged actions must be auditable, and external provisioning must be safe to retry.

## Status Legend

- **Complete**: Implemented and validated against its acceptance criteria.
- **In progress**: Partially implemented or actively being worked on.
- **Planned**: Required for the first production release but not started.
- **Future**: Useful expansion that is not required for the first production release.

## Current Baseline

| Capability | Status | Current behavior |
| --- | --- | --- |
| Discord command framework | Complete | Guild slash commands and component interactions run through Poise and Serenity. |
| Environment configuration | Complete | Discord, SMTP, Authentik, onboarding, and Postgres settings load from environment variables. |
| Structured logging | Complete | Startup and core workflow operations use `tracing`. |
| Postgres connection and migrations | Complete | The application opens a SQLx pool and runs embedded migrations during startup. |
| Email verification flow | Complete | Pending attempts, retry counts, expiry, and successful identities are persisted transactionally in Postgres. |
| Verified identity persistence | Complete | Successful identities are upserted in Postgres and the verification panel checks persisted identities. |
| Discord verified role assignment | Complete | The verified role is assigned after a successful code check. |
| Officer-reviewed onboarding | In progress | Requests, decisions, review message IDs, provisioning state, retries, and audit events are durable; private-server restart acceptance is still required. |
| Authentik provisioning | In progress | The client authenticates with the service account, finds or creates users, ensures group membership, and exposes an officer-only dependency check; live production validation remains. |
| Headscale onboarding | In progress | Target-specific completion URLs are sent only after Authentik group assignment; the Authentik-group-plus-login-URL boundary still needs live acceptance. |
| Authorization | Complete | Target-specific Discord roles are checked at command, decision, and provisioning execution time; target verification and persisted email are rechecked before approval/retry. |
| Audit history | In progress | Onboarding creation, decisions, review synchronization, provisioning attempts, failures, and completion are durable and inspectable; verification events still rely on structured logs. |
| Deployment and health checks | In progress | A production image and Postgres Compose deployment exist, including optional onboarding configuration; health/readiness endpoints remain planned. |

## Milestone 1: Durable Verification

Goal: verification remains correct across restarts and multiple bot instances.

### 1.1 Persist verified identities — Complete

- Store Discord user ID, verified email, and verification timestamp in Postgres.
- Update an existing identity when a user verifies again.
- Read the persisted identity when determining whether a user is already verified.
- Convert Discord `u64` identifiers safely at the Postgres boundary.

### 1.2 Persist pending verification attempts — Complete

- Store the Discord user ID, email, protected verification-code value, expiration time, and failed-attempt count.
- Reuse an unexpired attempt without sending unnecessary duplicate email.
- Expire attempts based on database timestamps rather than process uptime.
- Delete or mark an attempt consumed after successful verification.
- Preserve retry limits across restarts and concurrent requests.
- Do not store verification codes in plaintext.

### 1.3 Make verification state transitions reliable — Complete

- Treat accepting a code, recording the verified identity, and consuming the attempt as one database transaction.
- Define recovery behavior if database persistence succeeds but Discord role assignment fails, or vice versa.
- Allow a safe retry without issuing duplicate state or permanently stranding the user.
- Remove the in-memory verification and verified-identity stores after all consumers use repositories.

### 1.4 Verification abuse controls — Planned

- Restrict verification to approved email domains or another explicit eligibility policy.
- Rate-limit attempts by Discord user and email address.
- Avoid exposing verification codes or credentials in logs and user-facing errors.
- Define a re-verification and identity-change policy.

## Milestone 2: Durable Onboarding

Goal: onboarding requests can be reviewed and completed after restarts without losing state or duplicating access.

### 2.1 Persist onboarding requests — Complete

- Store request ID, target user, requesting officer, verified email, target key, request timestamp, status, and acting approver.
- Store Discord review channel and message IDs so the review message can be reconciled later.
- Enforce at most one pending request per user and target in the database.
- Read verified identities from Postgres instead of the temporary in-memory store.

### 2.2 Make approval and denial atomic — Complete

- Perform pending-to-approved and pending-to-denied transitions with conditional database updates.
- Prevent two officers or two bot instances from handling the same request twice.
- Return a clear already-handled response when a stale button is used.
- Preserve the actor and timestamp for every terminal decision.

### 2.3 Recover review interactions — In progress

- Resolve button interactions from persisted request state after a restart.
- Reconcile the Discord review message with the authoritative database status.
- Provide an officer command to inspect pending and recently handled requests.
- Define behavior for deleted review messages, removed targets, and removed users.

## Milestone 3: Reliable Provisioning

Goal: an approved request produces the intended external access exactly once, with failures visible and recoverable.

### 3.1 Finalize the Authentik integration — Planned

- Confirm and document the supported Authentik authentication method for the deployed service account.
- Validate user lookup, user creation, and group membership against the deployed Authentik instance.
- Treat existing users and existing group membership as successful idempotent outcomes.
- Classify retryable failures separately from permanent configuration or authorization failures.

### 3.2 Track provisioning state — Complete

- Separate officer approval from provisioning completion in the request state model.
- Record attempts, external identifiers, completion timestamps, and sanitized failure details.
- Permit safe retries without creating duplicate Authentik users or memberships.
- Do not mark a request fully completed until every required target action succeeds.

### 3.3 Define the Headscale boundary — Planned

- Decide whether Authentik group membership plus the login URL is the complete Headscale workflow.
- If direct Headscale operations are required, place them behind a dedicated integration adapter.
- Record the result of each Headscale-related action in provisioning history.

### 3.4 User and officer feedback — In progress

- Keep the review message synchronized with approved, denied, failed, retrying, and completed states.
- Notify the user only when access is ready or when a clear follow-up action is required.
- Give officers an actionable error reference without leaking credentials or sensitive responses.

## Milestone 4: Authorization and Audit

Goal: every privileged action is authorized at execution time and can be reconstructed later.

### 4.1 Durable audit events — In progress

- Record actor, target user, action, target system, request ID, timestamp, outcome, and safe metadata.
- Audit verification completion, onboarding creation, approval, denial, provisioning attempts, retries, and administrative actions.
- Make audit history append-only through the application interface.
- Provide an officer-only way to inspect history for a user or request.

### 4.2 Discord authorization — Complete

- Keep runtime role checks authoritative for commands and component interactions.
- Re-check authorization when an action is executed rather than relying on who can see a command.
- Document manual command-visibility configuration in Discord Server Settings.
- Decide whether an administrator OAuth flow is worth adding for automatic command visibility; it is not required if manual configuration is sufficient.

### 4.3 Administrative controls — In progress

- Add commands to inspect workflow status and retry failed provisioning.
- Define cancellation and revocation behavior.
- Require explicit role configuration for every privileged command.
- Log all administrative actions to the durable audit trail.

## Milestone 5: Production Readiness

Goal: the service can be deployed, monitored, upgraded, and recovered predictably.

### 5.1 Health and readiness — Planned

- Add a minimal HTTP server with liveness and readiness endpoints.
- Readiness must reflect required dependencies such as Postgres and Discord gateway state.
- Keep future OAuth callbacks and webhooks separate from the health surface.

### 5.2 Deployment — Planned

- Provide a production container image and documented runtime configuration.
- Document migration behavior, required Discord intents and permissions, SMTP setup, Authentik credentials, and Postgres requirements.
- Support graceful shutdown of the Discord client, HTTP server, and database pool.
- Define database backup and restore expectations before production use.

### 5.3 Validation and delivery — Planned

- Run formatting, Clippy, and automated tests in CI.
- Add database integration coverage for migrations, repositories, uniqueness, and concurrent state transitions.
- Exercise verification and onboarding end to end in a private Discord test server.
- Validate restart recovery during pending verification, pending review, and failed provisioning.
- Document a release and rollback procedure.

### 5.4 Observability — Planned

- Use stable request or correlation IDs across Discord, database, and external integration logs.
- Add useful metrics for verification outcomes, pending requests, provisioning failures, and dependency health.
- Define alerting for repeated startup, database, email, and provisioning failures.
- Ensure logs and metrics do not contain verification codes, passwords, tokens, or unnecessary personal data.

### 5.5 Privacy and data lifecycle — Planned

- Present the collected account name consistently as a preferred name rather than a legal or verified name.
- Tell members what identity data is stored, why it is needed, who can access it, and where it is sent.
- Define retention periods for verification identities, onboarding requests, audit events, application logs, and backups.
- Provide documented correction, offboarding, deletion, and incident-response procedures for personal data.
- Restrict personal data in Discord channels, Postgres, logs, and backups to authorized operators with a demonstrated need.
- Confirm the final data classification and handling expectations with RIT Information Security before declaring production readiness.

## First Production Release Definition of Done

The first production release is complete when all of the following are true:

- Milestones 1 through 5 have no remaining required items.
- Verification and onboarding state survive process restarts and concurrent bot instances.
- Approval and provisioning operations are idempotent and safely retryable.
- Runtime authorization protects every privileged operation.
- Durable audit history can explain who performed each action and what happened.
- Operators can determine whether the service and its dependencies are healthy.
- Deployment, configuration, backup, recovery, release, and rollback steps are documented.
- A private-server acceptance run proves the complete verification-to-access workflow.

## Future Features

These are intentionally outside the first production release unless promoted into a milestone:

- GitHub organization invites and team membership.
- Additional infrastructure targets through dedicated adapters.
- Ticket creation and external request tracking.
- OAuth-based verification or Discord Linked Roles.
- Self-service access requests where policy allows them.
- Access expiration, periodic reconciliation, and automated revocation.
- A web dashboard for officers and audit reporting.
- Multi-guild support.

## How to Update This Roadmap

When adding a feature, document:

1. The user or operator outcome.
2. Whether it is required for the first production release.
3. Its dependencies and failure boundaries.
4. Concrete acceptance criteria that can be demonstrated.
5. Its current status based on repository and runtime evidence.

A feature should move to **Complete** only after its acceptance criteria have been implemented and validated. Code existing in a branch is not enough by itself when the feature requires database, Discord, email, or external-system behavior.
