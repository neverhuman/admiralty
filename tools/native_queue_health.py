"""Read-only native queue health. Queue persistence never proves consumption.

This module performs no dispatch, UI input, provider calls, database mutation,
session resume, deletion, or retry. Missing/incomplete evidence stays unknown.
"""
import pathlib
import sqlite3
import time


def read_queue_health(path, *, now_ms=None, max_threads=128, stale_ms=300_000):
    path = pathlib.Path(path).resolve()
    now_ms = int(time.time() * 1000) if now_ms is None else now_ms
    if not 1 <= max_threads <= 128 or stale_ms <= 0:
        raise ValueError("invalid queue observation bounds")
    report = {"status": "unknown", "complete": False, "threads": [],
              "observed_ms": now_ms, "source": str(path),
              "consumption_verified": False, "read_only": True}
    if not path.is_file():
        report["reason"] = "queue database missing; no empty-queue inference"
        return report
    connection = None
    try:
        # mode=ro deliberately retains SQLite's WAL handling; immutable=1 would
        # miss a live writer's committed WAL rows. Never copy/read payload_json.
        connection = sqlite3.connect(path.as_uri() + "?mode=ro", uri=True,
                                     timeout=0.25)
        connection.execute("PRAGMA query_only=ON")
        connection.execute("PRAGMA busy_timeout=250")
        budget = [0]

        def bounded_query():
            budget[0] += 1
            return budget[0] > 1000  # at most about one million SQLite VM steps

        connection.set_progress_handler(bounded_query, 1000)
        rows = connection.execute(
            "SELECT thread_id, COUNT(*), MIN(created_at_ms), MAX(created_at_ms) "
            "FROM queued_items GROUP BY thread_id ORDER BY MIN(created_at_ms), thread_id LIMIT ?",
            (max_threads + 1,)).fetchall()
        report["complete"] = len(rows) <= max_threads
        for thread_id, count, oldest, newest in rows[:max_threads]:
            valid_clock = (isinstance(oldest, int) and isinstance(newest, int)
                           and 0 <= oldest <= newest <= now_ms)
            age = now_ms - oldest if valid_clock else None
            state = ("clock_unknown" if not valid_clock else
                     "stalled" if age >= stale_ms or count >= 16 else "pending")
            report["threads"].append({
                "thread_id": thread_id, "pending": count, "oldest_created_ms": oldest,
                "newest_created_ms": newest, "oldest_age_ms": age, "state": state,
                "consumption_verified": False})
        report["status"] = "observed" if report["complete"] else "incomplete"
    except (sqlite3.Error, OSError, ValueError) as error:
        report["reason"] = type(error).__name__ + ": queue state unavailable"
        report["threads"] = []
        report["status"] = "unknown"
        report["complete"] = False
    finally:
        if connection is not None:
            connection.close()
    return report


def require_live_target(thread_id, cwd, thread, turn):
    """Preflight only; never resume a notLoaded thread to make this check pass.

    A caller must additionally bind the live process incarnation and its actual
    endpoint. A transport acceptance remains distinct from recipient consumption.
    """
    if thread.get("id") != thread_id or thread.get("cwd") != cwd:
        raise ValueError("recipient identity/cwd mismatch")
    status = thread.get("status", {}).get("type")
    if status == "notLoaded":
        raise ValueError("recipient not loaded on this endpoint; do not resume or queue as fallback")
    if status != "active" or not turn or turn.get("status") != "inProgress" or not turn.get("id"):
        raise ValueError("no verified active turn for same-turn steering")
    return turn["id"]
