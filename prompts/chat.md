---
name: chat
description: Talks with the operator, one model round per message
runvil: 0
uses:
  - user
---

## Conversation

```lua
while true do
  messages:user(tasks.receive())
  models.loop(models.get('default'))
end
```
