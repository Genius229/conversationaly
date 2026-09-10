-- Meeting-level authority and GigaSTT result details.  Transcript rows remain
-- compatible with the existing UI and summary repositories.
CREATE TABLE IF NOT EXISTS meeting_transcript_metadata (
    meeting_id TEXT PRIMARY KEY,
    provenance TEXT NOT NULL CHECK (provenance IN ('draft_live', 'final_gigastt')),
    result_metadata TEXT,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE
);
