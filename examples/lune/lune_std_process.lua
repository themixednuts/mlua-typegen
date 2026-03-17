---@meta

---@alias ProcessSpawnOptionsStdioKind "default" | "forward" | "inherit" | "none"

---@class Child
---@field stdin ChildWriter (readonly)
---@field stdout ChildReader (readonly)
---@field stderr ChildReader (readonly)
local Child = {}

function Child:kill() end

---@async
---@return table
function Child:status() end

---@class ChildReader
local ChildReader = {}

---@async
---@param size integer?
---@return string?
function ChildReader:read(size) end

---@async
---@return string
function ChildReader:readToEnd() end

---@class ChildWriter
local ChildWriter = {}

---@async
---@param data string
function ChildWriter:write(data) end

---@async
function ChildWriter:close() end
