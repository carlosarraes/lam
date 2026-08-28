-- Paging orders by created_at alone, which items_status_created cannot serve: its leading
-- column is unconstrained, so every page would be a full scan plus a temp sort.
CREATE INDEX items_created ON items(created_at);
