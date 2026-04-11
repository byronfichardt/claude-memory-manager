#!/usr/bin/env python3
"""
Seed a fresh Claude Memory Manager SQLite database with generic fake memories
for demo purposes (screenshots, docs). No personal data.

Usage:
    python3 demo-seed.py /path/to/memories.db

The script creates the schema to match src-tauri/src/store/mod.rs migration v1
and inserts a realistic-looking set of topics + memories.
"""

from __future__ import annotations

import hashlib
import sqlite3
import sys
import time
import uuid


SCHEMA = r"""
CREATE TABLE schema_version (version INTEGER PRIMARY KEY);

CREATE TABLE topics (
    name TEXT PRIMARY KEY,
    description TEXT,
    color TEXT,
    created_at INTEGER NOT NULL
);

CREATE TABLE memories (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    content TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    memory_type TEXT,
    topic TEXT,
    source TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    access_count INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY(topic) REFERENCES topics(name) ON DELETE SET NULL
);

CREATE INDEX idx_memories_topic ON memories(topic);
CREATE INDEX idx_memories_updated ON memories(updated_at);
CREATE INDEX idx_memories_hash ON memories(content_hash);

CREATE VIRTUAL TABLE memories_fts USING fts5(
    title,
    description,
    content,
    content='memories',
    content_rowid='rowid',
    tokenize='porter unicode61'
);

CREATE TRIGGER memories_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, title, description, content)
    VALUES (new.rowid, new.title, new.description, new.content);
END;

CREATE TRIGGER memories_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, title, description, content)
    VALUES ('delete', old.rowid, old.title, old.description, old.content);
END;

CREATE TRIGGER memories_au AFTER UPDATE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, title, description, content)
    VALUES ('delete', old.rowid, old.title, old.description, old.content);
    INSERT INTO memories_fts(rowid, title, description, content)
    VALUES (new.rowid, new.title, new.description, new.content);
END;
"""


TOPICS = [
    "deployment",
    "testing",
    "workflow",
    "user-profile",
    "safety",
    "dev-practices",
    "projects",
]


MEMORIES: list[dict] = [
    # deployment (5)
    {
        "title": "Use docker compose for local development",
        "description": "Keep service definitions in docker-compose.yml so the whole team gets the same local environment",
        "type": "project",
        "topic": "deployment",
        "content": """Use `docker compose up` for local dev. All services (db, redis, worker, web) should be defined in `docker-compose.yml` and committed to the repo.

**Why:** New teammates get a working stack with one command. No more "works on my machine" setup drift.

**How to apply:**
- Put all dev services in `docker-compose.yml`
- Pin image versions — never use `:latest`
- Expose only the ports you need
- Mount source directories as volumes for live reload
""",
    },
    {
        "title": "Keep secrets in environment variables",
        "description": "Never commit credentials to the repo — use .env for local, env vars at deploy time for prod",
        "type": "feedback",
        "topic": "deployment",
        "content": """Credentials should NEVER be committed to the repository. Use `.env` files (gitignored) for local development and inject environment variables at deploy time in production.

**Why:** Leaked credentials in git history are permanent and expensive to rotate.

**How to apply:**
- Add `.env` and `.env.*` to `.gitignore`
- Commit a `.env.example` with placeholder values
- Use your deploy platform's secret store in production
- Rotate any credential that accidentally lands in git
""",
    },
    {
        "title": "Run migrations in CI before deploy",
        "description": "Database migrations should execute as part of the CI pipeline, before the deploy step",
        "type": "project",
        "topic": "deployment",
        "content": """Run database migrations as part of the CI pipeline, not as a manual step or post-deploy.

**Why:** Manual migrations are forgotten. Post-deploy migrations race the application boot — new code may start before the schema is ready.

**How to apply:**
- Add a `migrate` step to your CI pipeline after the build
- The migrate step should fail the deploy if migrations fail
- Keep migrations backward-compatible for at least one release cycle
""",
    },
    {
        "title": "Expose /health endpoints for every service",
        "description": "Load balancers need a health check to route traffic safely",
        "type": "project",
        "topic": "deployment",
        "content": """Every production service must expose a `/health` endpoint that returns `200 OK` when the service is ready to accept traffic.

The endpoint should verify:
- The process is up
- Database connection works
- Critical dependencies respond (cache, queue, etc.)

Load balancers and orchestrators use this for routing decisions — a failing health check takes the instance out of rotation.
""",
    },
    {
        "title": "Pin image versions in production",
        "description": "Never use :latest tags in production — always pin to specific versions",
        "type": "feedback",
        "topic": "deployment",
        "content": "Never use `:latest` image tags in production deployments. Always pin to a specific version or a content hash. `:latest` makes rollbacks impossible and deploys non-reproducible.",
    },
    # testing (3)
    {
        "title": "Keep unit tests under 30 seconds",
        "description": "Fast unit tests get run. Slow ones get skipped.",
        "type": "feedback",
        "topic": "testing",
        "content": """If your unit test suite takes longer than 30 seconds, split the slow ones out as integration tests.

**Why:** Developers won't run tests that are slow. Fast tests run on every save; slow tests run only in CI.

**How to apply:**
- Unit tests = pure logic, no I/O, no network
- Anything that touches a database or filesystem → integration suite
- Run unit tests in watch mode locally
""",
    },
    {
        "title": "Use snapshot tests for UI components",
        "description": "Snapshot tests catch unintended visual changes without manual review",
        "type": "project",
        "topic": "testing",
        "content": "Use snapshot tests for rendered UI components. They catch unintended visual changes without needing manual review. Update snapshots intentionally when the design changes.",
    },
    {
        "title": "Mock external services in tests",
        "description": "Tests should not hit real APIs — use mocks or fakes for determinism",
        "type": "feedback",
        "topic": "testing",
        "content": """Never hit real external APIs in tests. Use mocks, fakes, or recorded responses so tests stay deterministic and runnable offline.

**Why:** Real API tests are flaky (network, rate limits, upstream bugs), slow, and sometimes destructive.

**How to apply:**
- Use a mock library (nock, httpmock, wiremock, etc.)
- Record real responses once with a tool like VCR, commit them
- Have a separate (gated) integration suite for real API testing
""",
    },
    # workflow (3)
    {
        "title": "Keep PRs small and focused",
        "description": "Each PR should address one concern — small PRs review and revert cleanly",
        "type": "feedback",
        "topic": "workflow",
        "content": """Each pull request should address ONE concern. Small PRs:
- Review faster
- Merge faster
- Revert cleanly when something breaks
- Have shorter feedback loops

**How to apply:**
- If a PR description has "and" in it, split it
- Refactors go in their own PR, separate from behavior changes
- Target 200 lines or fewer when possible
""",
    },
    {
        "title": "Review your own PR first",
        "description": "Read the diff in the GitHub UI before requesting review",
        "type": "feedback",
        "topic": "workflow",
        "content": "Before requesting review, read through your own PR diff in the GitHub UI. Catch obvious issues yourself — save reviewer time and get faster turnaround. Also add inline comments to explain non-obvious decisions.",
    },
    {
        "title": "Commit messages explain WHY, not WHAT",
        "description": "The diff shows what changed — the commit message captures intent",
        "type": "feedback",
        "topic": "workflow",
        "content": """Commit messages should answer: **why did this change?**

The diff already shows what changed. A good message captures:
- The problem being solved
- Why this approach over alternatives
- Links to relevant issues or discussions

Bad: `update config`
Good: `cap worker concurrency at 4 to prevent OOM during batch jobs`
""",
    },
    # user-profile (2)
    {
        "title": "Developer profile",
        "description": "Background, stack, and working preferences",
        "type": "user",
        "topic": "user-profile",
        "content": """- **Primary stack:** TypeScript, Rust, Python
- **Editor:** Neovim with LSP integrations
- **Terminal:** tmux + zsh
- **Database:** PostgreSQL for most things, SQLite for local/embedded
- **Preferred container runtime:** Docker + docker compose

Prefers strict typing, explicit error handling, and fast iteration cycles.
""",
    },
    {
        "title": "Response style preferences",
        "description": "How the user prefers answers to be formatted",
        "type": "user",
        "topic": "user-profile",
        "content": """- Prefer concise, direct answers over long explanations
- Code samples should be runnable — no pseudo-code unless asked
- When proposing options, present tradeoffs honestly
- Don't hedge unnecessarily — pick the best approach and explain why
""",
    },
    # safety (2)
    {
        "title": "Back up databases before migrations",
        "description": "Take a snapshot before any production schema change",
        "type": "feedback",
        "topic": "safety",
        "content": """Always take a database snapshot before running a production migration. Keep the backup for at least 24 hours in case you need to roll back.

**Why:** Some migrations can't be reversed cleanly. Having a snapshot is the only way to recover if things go wrong.

**How to apply:**
- Automate the snapshot as part of the migration pipeline
- Verify the backup is restorable, not just that it ran
- Tag the snapshot with the migration version
""",
    },
    {
        "title": "Never force-push to main",
        "description": "Force push to shared branches is a hard rule",
        "type": "feedback",
        "topic": "safety",
        "content": "Force pushing to `main` (or any protected branch) is a hard no. Use merge commits or rebase locally before pushing a fresh branch. Configure GitHub branch protection rules to make it impossible.",
    },
    # dev-practices (2)
    {
        "title": "Lint on save",
        "description": "Configure the editor to run the linter on every save",
        "type": "feedback",
        "topic": "dev-practices",
        "content": "Configure your editor to run the linter on every save. Catching issues immediately is 10x faster than catching them in CI and having to context-switch back. Same for formatters.",
    },
    {
        "title": "Use auto-formatters, not manual alignment",
        "description": "Let the tool handle formatting — manual alignment is wasted review time",
        "type": "feedback",
        "topic": "dev-practices",
        "content": "Let Prettier, rustfmt, gofmt, or similar handle all formatting decisions. Manual alignment and style bikeshedding waste review time. Commit the formatter config, enforce in CI, move on.",
    },
    # projects (2)
    {
        "title": "Example dashboard",
        "description": "Internal metrics dashboard built with Vue 3 + Tailwind",
        "type": "project",
        "topic": "projects",
        "content": """A Vue 3 dashboard for tracking internal metrics. Uses:
- Vue 3 + Composition API
- Tailwind CSS for styling
- Pinia for state management
- Vite for the dev server

See the `/apps/example-dashboard` directory.
""",
    },
    {
        "title": "API gateway service",
        "description": "Go-based gateway fronting several microservices",
        "type": "project",
        "topic": "projects",
        "content": """A Go-based API gateway that fronts several microservices. Responsibilities:
- Rate limiting (token bucket, per-IP)
- Authentication middleware (JWT verification)
- Request tracing (OpenTelemetry)
- Routing to upstream services

Runs behind an ALB, exposes /health and /metrics endpoints.
""",
    },
]


def content_hash(content: str) -> str:
    return hashlib.sha256(content.encode("utf-8")).hexdigest()


def seed(db_path: str) -> None:
    conn = sqlite3.connect(db_path)
    try:
        # Schema must match store/mod.rs migration v1 exactly
        conn.executescript(SCHEMA)
        conn.execute("INSERT INTO schema_version (version) VALUES (1)")

        now = int(time.time())

        # Topics
        for name in TOPICS:
            conn.execute(
                "INSERT INTO topics (name, created_at) VALUES (?, ?)",
                (name, now),
            )

        # Memories
        for i, m in enumerate(MEMORIES):
            conn.execute(
                """INSERT INTO memories
                   (id, title, description, content, content_hash,
                    memory_type, topic, source, created_at, updated_at, access_count)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
                (
                    str(uuid.uuid4()),
                    m["title"],
                    m["description"],
                    m["content"],
                    content_hash(m["content"]),
                    m.get("type"),
                    m.get("topic"),
                    "demo_seed",
                    now - (i * 3600),
                    now - (i * 3600),
                    0,
                ),
            )

        conn.commit()
        print(f"Seeded {len(MEMORIES)} memories across {len(TOPICS)} topics")
    finally:
        conn.close()


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print("Usage: demo-seed.py /path/to/memories.db", file=sys.stderr)
        sys.exit(1)
    seed(sys.argv[1])
