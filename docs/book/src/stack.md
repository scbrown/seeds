# The stack

seeds is an experiment built on the caboodle stack. It is not one of the six
stack tools; it uses them.

| tool | what seeds uses it for |
|---|---|
| [quipu](https://github.com/scbrown/quipu) | the store: SPARQL reads, governed knot writes, transactions, qpacks |
| [camayoc](https://github.com/scbrown/camayoc) | the `WorkItem` shapes and the stored `ready` / `blocked` / plate queries |
| [caboodle](https://github.com/scbrown/caboodle) | installs seeds and proves it in `caboodle verify` |
| [desire-path](https://github.com/scbrown/desire-path) | redirects `bd` to seeds, and records every verb seeds refuses |
| [yupana](https://github.com/scbrown/yupana) | not a runtime dependency; the counting board seeds are named after |
| [bobbin](https://github.com/scbrown/bobbin) | not a runtime dependency |

A crew harness can also use seeds as a work-item tracker alongside its others,
through the same three-method tracker protocol. See [How it works](architecture.md).
