# TODO

## Recover changed Snap document-portal grants

Deployd currently detects when a persisted `/run/user/<uid>/doc/<id>/...` game or Wine-prefix
path is no longer accessible and asks the user to reselect it in Manage Games.

Investigate a durable portal-backed location reference that can survive document ID changes, or a
reauthorization workflow that opens the folder picker at the original host location. A Snap cannot
silently grant itself access to another Snap's hidden data, so any recovery design must preserve an
explicit user authorization step.
