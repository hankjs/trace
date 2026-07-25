from app import scheduler


def test_evening_pipeline_stops_when_any_bar_update_fails(monkeypatch):
    selection_called = False

    monkeypatch.setattr(scheduler, "_is_weekday", lambda *_: True)
    monkeypatch.setattr(
        scheduler, "job_daily_bars",
        lambda: {"skipped": False, "succeeded": 799, "failed": ["sh.600001"]},
    )

    def selection():
        nonlocal selection_called
        selection_called = True
        return {"picked": 30}

    monkeypatch.setattr(scheduler, "job_factors_and_selection", selection)
    scheduler.job_evening_pipeline()

    assert selection_called is False
