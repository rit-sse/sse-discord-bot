# SSE Discord Bot

The SSE Discord Bot is a Rust service for managing Discord-based onboarding, verification, and infrastructure access workflows for the SSE community. Discord is the primary user interface, but it should not become the source of truth for membership, verification, or infrastructure permissions. The bot coordinates user-facing Discord interactions with persisted workflow state and auditable integrations with SSE infrastructure systems.

See [ROADMAP.md](ROADMAP.md) for the current implementation status, release milestones, and definition of done.

The bot is built around `poise` and `serenity`. Poise provides the command framework for slash commands and typed command handlers, while Serenity provides lower-level Discord API access for guild events, roles, member state, and interactions. The service runs on Tokio, is incrementally moving workflow state into Postgres, and is planned to expose a small Axum web API for health checks and external callbacks before being deployed as a containerized service.

## Architecture

```mermaid
flowchart LR
    users["Members and Officers"]
    discord["Discord Server"]

    subgraph app["SSE Discord Bot"]
        direction TB
        commands["Poise Slash Commands"]
        events["Serenity Event Handlers"]
        web["Axum Web API"]
        services["Application Services"]
        dbaccess["Database Layer"]
        audit["Audit Logging"]

        commands --> services
        events --> services
        web --> services
        services --> dbaccess
        services --> audit
    end

    postgres[("Postgres")]

    subgraph external["External Systems"]
        direction TB
        identity["Identity Provider / Verification Source"]
        github["GitHub / Source Control"]
        infra["SSE Infrastructure APIs"]
        tickets["Ticketing / Request Tracking"]
    end

    users --> discord
    discord -- "slash commands" --> commands
    discord -- "gateway events" --> events
    discord -- "interaction callbacks" --> web

    dbaccess -- "workflow state" --> postgres
    audit -- "audit trail" --> postgres

    services -- "verify members" --> identity
    services -- "team access" --> github
    services -- "provision access" --> infra
    services -- "request tracking" --> tickets
```

The bot is divided into five main parts:

- **Commands:** Slash-command entry points for verification, onboarding, admin review, and infrastructure access requests.
- **Events:** Discord guild event handling for member joins, role changes, and lifecycle events that should trigger workflow updates.
- **Services:** Business logic for verification, onboarding, approval flows, and infrastructure provisioning.
- **Database:** Postgres-backed persistence for workflow state, verification attempts, access requests, and audit history.
- **Web API:** Axum endpoints for health checks, OAuth callbacks, webhooks, and future Discord Linked Roles support.

## Planned Stack

| Concern | Choice | Purpose |
| --- | --- | --- |
| Language | Rust | Reliable async service with strong type safety |
| Discord framework | Poise | Slash commands and ergonomic command handlers |
| Discord API | Serenity | Gateway events, roles, guild members, and lower-level API access |
| Runtime | Tokio | Async runtime for Discord, web, and database work |
| Web API | Axum | Health checks, callbacks, OAuth redirects, and webhooks |
| Database | Postgres | Durable workflow, verification, and audit state |
| SQL access | SQLx | Compile-time checked SQL and migrations |
| Configuration | Environment variables, `config`, `dotenvy` | Local and production configuration |
| Secrets | `secrecy` and deployment-managed secrets | Discord tokens, database URLs, OAuth credentials |
| Observability | `tracing` | Structured logs and operational debugging |
| Deployment | Docker/container first | Portable deployment to SSE-managed infra or common hosts |

## Core Workflows

### Verification

Verification starts in Discord with a slash command or button-driven interaction. The bot records the verification attempt, checks the user against an approved verification source, and assigns the verified role only after the workflow succeeds.

Initial verification can be implemented with a Discord-native flow using ephemeral responses, buttons, and modals. Stronger checks can be added later through OAuth, invite codes, CAPTCHA-backed web flows, or Discord Linked Roles.

### Onboarding

Onboarding is modeled as an explicit workflow instead of a one-off command. A member requests access, the bot persists the request, officers review it, and approved actions are executed through service adapters.

Examples of future onboarding actions include GitHub organization invites, team membership updates, cloud or lab infrastructure access requests, documentation links, and ticket creation.

### Admin and Audit

Privileged actions should require explicit officer/admin commands and should be written to an audit trail. The audit trail should include the Discord actor, target user, action type, timestamp, request state, and external system result when applicable.

## Design Principles

- Prefer slash commands and Discord interactions over message-prefix commands.
- Avoid requiring the Discord message content privileged intent unless a future feature truly needs it.
- Keep Discord as the interface, not the database of record.
- Keep infrastructure-specific actions behind service adapters.
- Make verification, onboarding, and role changes replayable, reviewable, and auditable.
- Keep deployment portable; do not bake Shuttle or any single hosting platform into core application logic.

## Runtime Features

`BOT_FEATURES` explicitly controls which modules are initialized and exposed. Disabled modules do not load their configuration, construct integration clients, register commands, or process their interactions.

| Feature | Commands | Required configuration |
| --- | --- | --- |
| `age` | `/age` | Core configuration only |
| `verification` | `/verify` | `VERIFIED_ROLE_ID` and SMTP configuration |
| `onboarding` | `/onboard` and onboarding components | Verification plus Authentik and onboarding configuration |

Onboarding depends on verification, and startup rejects configurations that enable onboarding alone. To run only the email-verification workflow, set:

```dotenv
BOT_FEATURES=verification
```

To run every currently implemented feature, set:

```dotenv
BOT_FEATURES=age,verification,onboarding
```

## Local Development

Copy the example environment file and fill in the values for your development services:

```sh
cp .env.example .env
```

The example enables only email verification. Configure the core, database, verified-role, and SMTP variables; Authentik and onboarding variables are not required in this mode.

When onboarding is enabled, configure `AUTHENTIK_USERNAME` and `AUTHENTIK_PASSWORD` with a dedicated Authentik service account and app password. `AUTHENTIK_CLIENT_ID` identifies the OAuth2 provider used for machine-to-machine login. That provider must expose the `goauthentik.io/api` scope, and the service account should have only the permissions needed to view and create users and add users to the configured groups. The bot exchanges those credentials for a short-lived access token; it does not store a static Authentik API token.

Run the migrations and start the bot:

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
sqlx migrate run
cargo run
```

The bot also runs embedded migrations during startup. See [.env.example](.env.example) for the complete configuration surface.

## Testing Strategy

- Unit test verification and onboarding state machines without making Discord API calls.
- Integration test database repositories and migrations against Postgres.
- Add smoke tests for Axum health and callback endpoints.
- Mock Discord-facing role assignment and interaction boundaries.
- Manually test bot behavior in a private Discord test server before production use.
