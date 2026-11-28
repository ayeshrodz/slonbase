slonbase 2.7.1 against PocketBase 0.40.6 and PostgREST 16.5: median of 5 runs of 10s per scenario, 8 CPUs (± is half the spread between runs)

| Scenario | Clients | slonbase 2.7.1 req/s | PocketBase 0.40.6 req/s | slonbase / PocketBase | PostgREST 16.5 req/s | slonbase / PostgREST | p50 ms (slonbase / PB / PostgREST) | p99 ms (slonbase / PB / PostgREST) |
|---|--:|--:|--:|--:|--:|--:|--:|--:|
| health | 32 | 13,003 ±0% | 11,570 ±1% | 1.12× | 12,325 ±0% | 1.05× | 2.40 / 2.40 / 2.60 | 5.30 / 10.7 / 5.80 |
| view one record | 32 | 6,217 ±1% | 4,835 ±4% | 1.29× | 1,848 ±4% | 3.37× | 5.00 / 3.80 / 17.5 | 9.90 / 41.7 / 29.0 |
| view one record (1 client) | 1 | 822 ±1% | 1,500 ±4% | 0.55× | 644 ±2% | 1.28× | 1.20 / 0.60 / 1.50 | 1.40 / 1.60 / 2.70 |
| list 30 | 32 | 2,254 ±1% | 513 ±2% | 4.40× | 1,371 ±3% | 1.64× | 14.0 / 55.5 / 23.0 | 23.8 / 185.2 / 31.8 |
| list 30, sorted | 32 | 2,104 ±1% | 502 ±1% | 4.19× | 1,253 ±5% | 1.68× | 15.1 / 56.8 / 25.3 | 25.4 / 175.1 / 35.0 |
| list 30, filtered | 32 | 2,067 ±2% | 57.4 ±1% | 36.03× | 1,214 ±0% | 1.70× | 15.4 / 552.6 / 26.2 | 25.6 / 820.1 / 35.0 |
| list 30, expand author | 32 | 1,209 ±2% | 372 ±7% | 3.25× | n/a | – | 26.2 / 74.7 / n/a | 38.2 / 254.3 / n/a |
| list 30, signed in | 32 | 1,764 ±3% | 485 ±5% | 3.64× | n/a | – | 17.8 / 60.2 / n/a | 28.3 / 157.2 / n/a |
| create record | 32 | 2,575 ±1% | 2,457 ±6% | 1.05× | 1,436 ±0% | 1.79× | 12.2 / 8.70 / 22.1 | 18.5 / 58.7 / 29.6 |
| sign in (bcrypt) | 8 | 10.9 ±0% | 10.9 ±0% | 1.00× | n/a | – | 738.6 / 722.8 / n/a | 840.6 / 866.4 / n/a |
