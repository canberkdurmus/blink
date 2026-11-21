-- Interop probe for `cargo xtask interop` (SPEC §6, §12 nightly): run inside darktable with
--   darktable --library :memory: --luacmd "dofile('<this file>')"
-- Imports every non-XMP file of each folder in $BLINK_DT_DIRS (separated by ';') and writes
-- one line per image to $BLINK_DT_OUT: `<folder>/<file> rating=<n> label=<colour|none>`,
-- i.e. what darktable itself read from the sidecars. Then quits.
local dt = require "darktable"
local out = assert(io.open(os.getenv("BLINK_DT_OUT"), "w"))
for dir in string.gmatch(os.getenv("BLINK_DT_DIRS"), "[^;]+") do
  local names = {}
  for name in io.popen('ls "' .. dir .. '"'):lines() do
    if not name:lower():match("%.xmp$") then table.insert(names, name) end
  end
  table.sort(names)
  for _, name in ipairs(names) do
    local img = dt.database.import(dir .. "/" .. name)
    if img then
      local labels = {}
      for _, c in ipairs({ "red", "yellow", "green", "blue", "purple" }) do
        if img[c] then table.insert(labels, c) end
      end
      local label = #labels > 0 and table.concat(labels, ",") or "none"
      out:write(string.format("%s/%s rating=%d label=%s\n", dir, name, img.rating, label))
    else
      out:write(string.format("%s/%s import-failed\n", dir, name))
    end
  end
end
out:close()
os.exit(0)
