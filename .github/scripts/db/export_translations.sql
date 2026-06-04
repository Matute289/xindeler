-- Export all translation data from DB back to ci-data/translations.csv.
-- Run from the repo root so relative path resolves correctly.
\copy (
  SELECT country_code, file_name, translation_key, status, git_commit
  FROM translations
  ORDER BY country_code, file_name, translation_key
) TO 'ci-data/translations.csv' CSV HEADER;
