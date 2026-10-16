# Retired three-arm tasks

Tasks kept so the historical records in [`docs/three-arm/`](../../docs/three-arm/) stay reproducible. They are not part of the active suite in `fixtures/appport-tasks/`.

| task | why it was retired | replaced by |
|---|---|---|
| `billing-from-store` | **Invalid as a measurement.** Its oracle verified preservation only: nothing observable distinguished "reads the store" from "unchanged", so an unchanged legacy lookup passed. Its contract also declared `requires: customers.lookup@1`, which contradicts the goal. See [A flawed task](../../docs/three-arm-eval.md#a-flawed-task-billing-from-store). | `billing-migration`, whose contract allows the change and whose oracle runs a goal check that the unchanged fixture fails |
