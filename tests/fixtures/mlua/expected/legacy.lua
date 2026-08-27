---@meta

---@class (exact) Handwritten
---@field value integer
local Handwritten = {}

---@return string
Handwritten["__tostring"] = function(self) end

---@param value integer
---@return integer
Handwritten["add"] = function(self, value) end

---@param value integer
---@return Handwritten
Handwritten["new"] = function(value) end

---@return integer
Handwritten["read"] = function(self) end

---@async
---@return integer
Handwritten["read_async"] = function(self) end

---@class (exact) RuntimeRegistered
---@field enabled boolean (readonly)
local RuntimeRegistered = {}

---@return boolean
RuntimeRegistered["toggle"] = function(self) end

---@class (exact) __mlua_21x636f6d7061746962696c6974795f66697874757265
---@field answer integer
---@type __mlua_21x636f6d7061746962696c6974795f66697874757265
_G["compatibility_fixture"] = _G["compatibility_fixture"] or {}

---@class (exact) __mlua_15x72656e616d65645f66697874757265
---@field enabled boolean
---@type __mlua_15x72656e616d65645f66697874757265
_G["renamed_fixture"] = _G["renamed_fixture"] or {}
