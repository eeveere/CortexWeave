ALTER TABLE documents
ADD COLUMN segmentation_id TEXT NOT NULL DEFAULT 'legacy-v0.1';
