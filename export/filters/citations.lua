-- stylos-export citation filter.
--
-- Replaces links whose target starts with `nostos:cite/` with a footnote.
-- The footnote text comes from `doc.meta.citations`, a map written by
-- stylos-export and passed with --metadata-file:
--
--   {"citations": {"<id>": {"markdown": "Author, Book, p. 12", "url": "..."}}}
--
-- When the cached library has no entry for an id, the link label is used, so
-- a citation is never lost.

local PREFIX = "nostos:cite/"

local function as_text(value)
  if value == nil then
    return ""
  end
  if type(value) == "string" then
    return value
  end
  if type(value) == "number" then
    return tostring(value)
  end
  local ok, text = pcall(pandoc.utils.stringify, value)
  if ok and text then
    return text
  end
  return ""
end

local function citation_map(meta)
  local citations = {}
  local raw = meta and meta.citations or nil
  if raw == nil then
    return citations
  end
  for id, entry in pairs(raw) do
    local text = ""
    if type(entry) == "table" and entry.markdown ~= nil then
      text = as_text(entry.markdown)
    else
      text = as_text(entry)
    end
    if text ~= "" then
      citations[tostring(id)] = text
    end
  end
  return citations
end

local function target_id(target)
  if target == nil then
    return nil
  end
  if target:sub(1, #PREFIX) ~= PREFIX then
    return nil
  end
  local id = target:sub(#PREFIX + 1)
  id = id:gsub("[?#].*$", "")
  if id == "" then
    return nil
  end
  return id
end

local function note_for(text, label)
  local blocks
  if text ~= nil and text ~= "" then
    blocks = pandoc.read(text, "markdown").blocks
  end
  if blocks ~= nil and #blocks > 0 then
    return pandoc.Note(blocks)
  end
  if label ~= nil and #label > 0 then
    return pandoc.Note({ pandoc.Para(label) })
  end
  return pandoc.Note({ pandoc.Para({ pandoc.Str("Citation") }) })
end

function Pandoc(doc)
  local citations = citation_map(doc.meta)
  local filter = {
    Link = function(link)
      local id = target_id(link.target)
      if id == nil then
        return nil
      end
      return note_for(citations[id], link.content)
    end,
  }
  -- Walk the whole document once. `doc.blocks` is a Blocks object, not a Lua
  -- array, so wrapping it in a Div and walking that is the portable way to
  -- reach every inline.
  local wrapper = pandoc.Div(doc.blocks)
  doc.blocks = pandoc.walk_block(wrapper, filter).content
  return doc
end
