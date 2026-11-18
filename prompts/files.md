---
name: files
description: Talks with the operator and works on the run's files
runvil: 0
uses:
  - user
  - std
---

## Conversation

```lua
tools.offer('std')
while true do
  messages:user(tasks.receive())
  models.loop(models.get('default'))
end
```
