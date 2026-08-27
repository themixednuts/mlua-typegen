---@meta

---@class (exact) Attributed
---@field value integer (readonly)
local Attributed = {}

---@return string
Attributed["__tostring"] = function(self) end

---@param amount integer
---@return integer
Attributed["increment"] = function(self, amount) end

---@class (exact) Generated
---@field count integer (readonly)
---@field display_name string
local Generated = {}

---@class (exact) __mlua_21x636f6d7061746962696c6974795f66697874757265
---@field answer integer
---@type __mlua_21x636f6d7061746962696c6974795f66697874757265
_G["compatibility_fixture"] = _G["compatibility_fixture"] or {}

---@class (exact) __mlua_15x72656e616d65645f66697874757265
---@field enabled boolean
---@type __mlua_15x72656e616d65645f66697874757265
_G["renamed_fixture"] = _G["renamed_fixture"] or {}
