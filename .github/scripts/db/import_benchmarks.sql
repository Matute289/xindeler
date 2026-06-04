-- Import historical benchmark data from ci-data/benchmarks.csv into the DB.
\copy benchmarks ("group", "function", value, throughput_num, throughput_type,
                  sample_measured_value, unit, iteration_count, git_commit, branch, recorded_at)
  FROM 'ci-data/benchmarks.csv' CSV HEADER;
