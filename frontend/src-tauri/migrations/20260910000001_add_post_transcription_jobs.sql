CREATE TABLE meeting_post_transcription_jobs (
    meeting_id TEXT PRIMARY KEY NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    run_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('running','ready','failed','cancelled','interrupted')),
    requires_final INTEGER NOT NULL DEFAULT 0 CHECK(requires_final IN (0,1)),
    error_code TEXT,
    updated_at TEXT NOT NULL
);
