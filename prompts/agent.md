---
name: agent
description: Talks with the operator, reads the mounted real files, and fetches web pages
runvil: 0
uses:
  - user
  - std
  - web
---

## Conversation

```lua
tools.offer('std/read')
tools.offer('std/dir')
tools.offer('std/grep')
tools.offer('web/fetch')
local root = ui().workspace_root
if root then
  messages:system('The directory ' .. root .. ' holds real files, mounted read-only. Give every tool an absolute path.')
end
while true do
  messages:user(tasks.receive())
  models.loop(models.get('default'))
end
```
