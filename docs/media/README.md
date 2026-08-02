# Product captures

These captures are generated from the real `NavigatorView::render` path using
privacy-safe fixture data. They demonstrate presentation only: no provider
terminal, prompt, response, filesystem path, host identity, credential, or
durable Workstream identifier is captured or committed.

| Capture | What it shows |
| --- | --- |
| [Workstreams](screenshots/workstreams.png) | The default Recent view: project-first context, activity age, lifecycle markers, and compact keyboard hints. |
| [Recovery and remote state](screenshots/remote-recovery.png) | Conservative failure presentation: recovery stays explicit and remote reachability does not fabricate a runtime stop. |
| [First Project](screenshots/first-project.png) | The empty-navigator onboarding state, which asks for explicit Project registration rather than inferring a checkout. |

Both SVG source captures and PNG previews are committed so the repository,
documentation host, and local previewer can use the appropriate format. To
refresh them after a presentation change, run this from the repository root:

```console
scripts/capture-docs
```

The generator is [examples/capture_docs.rs](../../examples/capture_docs.rs).
It requires ImageMagick's `magick` command only when rendering the PNG previews.
