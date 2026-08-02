from __future__ import annotations

import os
import secrets
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Any

import uvicorn
from fastapi import FastAPI, HTTPException, Request
from fastapi.responses import JSONResponse

from .deterministic import run_deterministic
from .durable import DurableRuntime
from .memory import ScientificMemory
from .models import (
    CapabilityState,
    ExperimentRecord,
    GoalIntent,
    PromotionDecision,
    PromotionEvaluationRequest,
    StoredExperiment,
    WorkflowResult,
)
from .promotion import evaluate_promotion


@asynccontextmanager
async def lifespan(app: FastAPI):  # type: ignore[no-untyped-def]
    enabled = os.getenv("RAMPAGE_ENABLE_MODELS", "false").casefold() == "true"
    app.state.runtime = None
    app.state.capability = CapabilityState.DETERMINISTIC_ONLY
    app.state.runtime_error = None
    data_dir = Path(os.getenv("RAMPAGE_DATA_DIR", ".rampage/runtime/intelligence"))
    app.state.memory = ScientificMemory(data_dir / "scientific-memory.sqlite")
    if enabled:
        try:
            model = os.getenv("RAMPAGE_MODEL", "ollama:qwen3:8b")
            app.state.runtime = DurableRuntime(data_dir=data_dir, model=model)
            app.state.capability = CapabilityState.FULL
        except Exception as error:  # fail into deterministic mode, never invent readiness
            app.state.runtime_error = str(error)
    yield
    app.state.memory.close()


app = FastAPI(
    title="Rampage Intelligence",
    version="0.2.0",
    description="Proposal-only durable intelligence outside the Rampage trust kernel",
    lifespan=lifespan,
)


@app.middleware("http")
async def require_local_token(request: Request, call_next):  # type: ignore[no-untyped-def]
    if request.url.path == "/health":
        return await call_next(request)
    expected = os.getenv("RAMPAGE_TOKEN")
    if not expected:
        return JSONResponse(
            status_code=503,
            content={"detail": "Rampage intelligence token is not configured"},
        )
    if not secrets.compare_digest(request.headers.get("x-rampage-token", ""), expected):
        return JSONResponse(
            status_code=401,
            content={"detail": "valid local Rampage token required"},
        )
    return await call_next(request)


@app.get("/health")
async def health(request: Request) -> dict[str, Any]:
    auth_configured = bool(os.getenv("RAMPAGE_TOKEN"))
    return {
        "service": "rampage-intelligence",
        "status": "ready" if auth_configured else "blocked",
        "capability": request.app.state.capability,
        "authority": "proposal_only",
        "auth_configured": auth_configured,
        "model_error": request.app.state.runtime_error,
    }


@app.post("/v1/goals", response_model=WorkflowResult)
async def submit_goal(intent: GoalIntent, request: Request) -> WorkflowResult:
    runtime: DurableRuntime | None = request.app.state.runtime
    if runtime is None:
        return run_deterministic(intent)
    try:
        return await runtime.run(intent)
    except Exception as error:
        raise HTTPException(
            status_code=503,
            detail=f"durable model workflow unavailable: {error}",
        ) from error


@app.post("/v1/memory/experiments", response_model=StoredExperiment)
async def record_experiment(
    experiment: ExperimentRecord, request: Request
) -> StoredExperiment:
    memory: ScientificMemory = request.app.state.memory
    try:
        return memory.record(experiment)
    except ValueError as error:
        raise HTTPException(status_code=409, detail=str(error)) from error


@app.get("/v1/memory/experiments/{digest}", response_model=StoredExperiment)
async def get_experiment(digest: str, request: Request) -> StoredExperiment:
    memory: ScientificMemory = request.app.state.memory
    experiment = memory.get(digest)
    if experiment is None:
        raise HTTPException(status_code=404, detail="experiment is absent")
    return experiment


@app.post("/v1/improvements/evaluate", response_model=PromotionDecision)
async def improvement_eligibility(
    evaluation: PromotionEvaluationRequest,
) -> PromotionDecision:
    return evaluate_promotion(evaluation.proposal, envelope=evaluation.envelope)


def main() -> None:
    uvicorn.run(
        "rampage_intelligence.api:app",
        host=os.getenv("RAMPAGE_INTELLIGENCE_HOST", "127.0.0.1"),
        port=int(os.getenv("RAMPAGE_INTELLIGENCE_PORT", "47832")),
        reload=False,
    )
