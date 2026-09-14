ALTER TABLE delivery_receipts ADD COLUMN explicit_retry INTEGER NOT NULL DEFAULT 0 CHECK(explicit_retry IN (0, 1));
