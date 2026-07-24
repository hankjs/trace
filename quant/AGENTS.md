# Repository Guidelines

## Project Structure & Module Organization

The FastAPI backend lives in `app/`. `app/main.py` wires the API and scheduler; route handlers are under `app/api/`; SQLAlchemy setup and `quant_*` models are in `app/db.py` and `app/models.py`. Market ingestion belongs in `app/data/` (including `universe.py` for the hs300/zz500 index pool), factor computation in `app/factors/`, stock selection in `app/selection/` (daily Top-30 pipeline + parameterised screener), strategy implementations in `app/strategy/strategies/` (single-name `positions()` and portfolio `target_weights()` contracts), portfolio logic in `app/portfolio/`, and simulation code in `app/backtest/` (vectorbt-based engine, parameter sweep, weekly batch evaluation). One-off scripts (pool backfill, index-membership history rebuild, lookahead check, engine regression baseline) live in `scripts/`.

The Vue 3 frontend is in `web/`. Page-level components live in `web/src/views/`, reusable components in `web/src/components/`, and shared API/types in `web/src/api.ts`. Production assets are generated into `web/dist/` and served by FastAPI when present; do not edit generated files.

## Build, Test, and Development Commands

- `uv sync`: create/update the Python environment from `pyproject.toml` and `uv.lock`.
- `uv run uvicorn app.main:app --port 8100 --reload`: run the backend with reload; Swagger UI is at `/docs`.
- `cd web && pnpm install`: install frontend dependencies from `pnpm-lock.yaml`.
- `cd web && pnpm dev`: start Vite on port 5173 and proxy `/api` to port 8100.
- `cd web && pnpm build`: run strict Vue/TypeScript checks and produce `web/dist/`.
- `curl http://localhost:8100/api/health`: smoke-test a running backend.

## Coding Style & Naming Conventions

Use four-space indentation, `snake_case` functions/modules, `PascalCase` classes, type hints, and focused docstrings in Python. Keep API handlers thin and put domain behavior in the existing data, strategy, portfolio, or backtest modules. Vue files use `<script setup lang="ts">`, two-space indentation, single quotes, no semicolons, PascalCase component filenames, and strict TypeScript. Reuse shared API interfaces and Tailwind theme tokens rather than duplicating request or color logic.

## Testing Guidelines

Backend regression tests use pytest: `uv run pytest tests/` (engine tests are synthetic and need no database). Before submitting, run `pnpm build`, exercise affected endpoints through `/docs` or `curl`, and verify database-backed flows against disposable data. New backend tests should use `tests/test_<feature>.py`. For frontend tests, use `*.spec.ts` and document the added runner.

## Commit & Pull Request Guidelines

History follows Conventional Commits, for example `feat: 添加量化` and `fix(client): ...`. Use `type(scope): imperative summary` where a scope adds clarity. Pull requests should describe behavior and database/API impact, list verification commands, link the issue, note configuration changes, and include screenshots for visible UI changes.

## Security & Configuration

Keep database URLs in the repository-level or local `config.toml`; both are ignored and must not be committed. Preserve the `quant_` table prefix and never add automatic trade execution: this project is an information and backtesting system only. Auth shares the server's `users` table (read-only, raw SQL in `app/auth.py`) and JWT secret: `jwt_secret` is read from the root `[server].jwt_secret` or overridden by `[quant].jwt_secret` — on the production host (no root config.toml) it must be set in `quant/config.toml`.

## Product Boundary: Daily Research, Manual Trading

This is a daily-frequency research and decision-support system. Strategies consume daily bars and produce informational picks or signals; backtest fills are simulations. Intraday snapshots are for display and valuation only. Never add broker connectivity, order submission, automatic execution, semi-automatic execution, or features that imply the system will trade on the user's behalf. All real buy, sell, position-sizing, and risk decisions are confirmed and executed manually by the user in an external trading application. Records in `quant_trade` are manual bookkeeping entries, not orders generated or executed by this system.
