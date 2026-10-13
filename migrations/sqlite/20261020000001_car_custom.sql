-- Milestone 17: paint jobs and performance parts (shared/src/vehicle.rs PAINTS and PARTS).
-- paint: an index into PAINTS, NULL for the factory color. parts: one bit per installed part.
ALTER TABLE cars ADD COLUMN paint INTEGER;
ALTER TABLE cars ADD COLUMN parts INTEGER NOT NULL DEFAULT 0;
