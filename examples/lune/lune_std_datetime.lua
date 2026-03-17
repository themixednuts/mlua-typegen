---@meta

---@class DateTime
---@field unixTimestamp integer (readonly)
---@field unixTimestampMillis integer (readonly)
local DateTime = {}

---@param other UserDataRef
---@return boolean
function DateTime:__eq(other) end

---@param other UserDataRef
---@return boolean
function DateTime:__lt(other) end

---@param other UserDataRef
---@return boolean
function DateTime:__le(other) end

---@return string
function DateTime:toIsoDate() end

---@return string
function DateTime:toRfc3339() end

---@return string
function DateTime:toRfc2822() end

---@param format string?
---@param locale string?
---@return string
function DateTime:formatUniversalTime(format, locale) end

---@param format string?
---@param locale string?
---@return string
function DateTime:formatLocalTime(format, locale) end

---@return DateTimeValues
function DateTime:toUniversalTime() end

---@return DateTimeValues
function DateTime:toLocalTime() end
