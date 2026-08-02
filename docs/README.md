# Workstream Navigator documentation

## Current contract

- [V1 design](design.md) is the product and architecture authority.
- [V1 roadmap](roadmap.md) owns delivery order, checkpoint status, and exit
  gates. D0 through D7.6 are complete; the source remains an operator beta,
  not a tagged distribution.
- The top-level [README](../README.md) is the product landing page and
  source-install quick start. `wsnav --help` and `wsnav host doctor <alias>`
  remain the authoritative runtime command and compatibility checks for an
  installed build.

## Evidence and research

The `acceptance-*.md` files are dated, sanitized records for the candidate and
environment stated in each file. Their pass/fail result, procedure, and
limitations are evidence; historical protocol/schema values, test counts,
shortcuts, and presentation descriptions do not replace the current design,
roadmap, or operator guide.

The `spikes/` and `studies/` directories record design falsification and
market/provider research. They inform the V1 boundary but are not a public API
or an implementation commitment by themselves.
