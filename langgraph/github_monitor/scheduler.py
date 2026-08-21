import logging

from apscheduler.schedulers.background import BackgroundScheduler
from apscheduler.triggers.cron import CronTrigger

from . import config, db, graph

logger = logging.getLogger("github_monitor")

_scheduler = None


def run_daily_report() -> str:
    # This activates the same pipeline as everything else; its log_input node writes the
    # trigger row and its gather_facts/summarize_report nodes fill in the agent_response.
    result = graph.run("daily_report")
    report_text = result.get("final_response", "")
    logger.info("Daily report generated:\n%s", report_text)
    return report_text


def start_scheduler():
    global _scheduler
    if _scheduler is not None:
        return _scheduler
    db.init_db()
    hour, minute = config.DAILY_REPORT_TIME.split(":")
    _scheduler = BackgroundScheduler()
    _scheduler.add_job(run_daily_report, CronTrigger(hour=int(hour), minute=int(minute)))
    _scheduler.start()
    logger.info("Daily report scheduler started, will run at %s every day", config.DAILY_REPORT_TIME)
    return _scheduler
