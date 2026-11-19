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
-- TODO kb should not need anything like this, if it's not working, need to fix the tool
local page = tools.get('web/fetch')
local function call(arguments)
  local ok, answer = pcall(tools.call, page, arguments)
  if ok then
    return answer
  end
  if type(answer) == 'table' and answer.kind == 'tool' then
    return untrusted(answer.message)
  end
  error(answer, 0)
end
local function fetch(arguments)
  local text = call(arguments)
  if #text > 2000 or not text:find('\ntruncated: false\nextraction: readability\n', 1, true) then
    return text
  end
  return call({ url = arguments.url, max_chars = arguments.max_chars, raw = true })
end
tools.offer('std/read')
tools.offer('std/dir')
tools.offer('std/grep')
tools.offer(tools.adhoc({ name = 'web_fetch', description = page.description, parameters = page.parameters }, fetch))
local guide = 'A fetched page holds only its main article when its extraction line says readability. When that text lacks what you look for, fetch the same URL again with raw set to true, which returns the whole page. Never repeat a call with the same arguments.'
local root = ui().workspace_root
if root then
  guide = 'The directory ' .. root .. ' holds real files, mounted read-only. Give every tool an absolute path. ' .. guide
end
-- TODO kb this can get duplicated in a DB — ask Claude to look at the engine and look for the duplicated data
-- consider compressing the DB data


-- TODO kb get even smaller model
-- generally, make my work fast, if I wait on something this is bad
messages:system(guide)
while true do
  local text = tasks.receive()
  if text:match('^%s*/draw%s*$') then
    pcall(log, 'drawn: ' .. math.random(1, 1000000))
  else
    messages:user(text)
    local ok, failure = pcall(models.loop, models.get('default'))
    if not ok then
      pcall(log, 'the reply ended early: ' .. tostring(failure.code))
    end
  end
end
```
