---@meta

---@class ServeHandle
---@field ip string (readonly)
---@field port integer (readonly)
local ServeHandle = {}

function ServeHandle:stop() end

---@class Request
---@field ip string? (readonly)
---@field port integer? (readonly)
---@field method string (readonly)
---@field path string (readonly)
---@field query table (readonly)
---@field headers table (readonly)
---@field body string (readonly)
local Request = {}

---@class Response
---@field ok boolean (readonly)
---@field statusCode integer (readonly)
---@field statusMessage string (readonly)
---@field headers table (readonly)
---@field body string (readonly)
local Response = {}

---@class Tcp
---@field localIp string? (readonly)
---@field localPort integer? (readonly)
---@field remoteIp string? (readonly)
---@field remotePort integer? (readonly)
local Tcp = {}

---@async
---@param size integer?
---@return string
function Tcp:read(size) end

---@async
---@param data string
function Tcp:write(data) end

---@async
function Tcp:close() end

---@class Websocket
---@field closeCode integer? (readonly)
local Websocket = {}

---@async
---@param code integer?
function Websocket:close(code) end

---@async
---@param string string
---@param as_binary boolean?
function Websocket:send(string, as_binary) end

---@async
---@return string?
function Websocket:next() end
