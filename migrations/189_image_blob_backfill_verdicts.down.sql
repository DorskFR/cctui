ALTER TABLE image_blob_backfill
    DROP COLUMN IF EXISTS images_valid,
    DROP COLUMN IF EXISTS images_corrupt,
    DROP COLUMN IF EXISTS images_recovered;
