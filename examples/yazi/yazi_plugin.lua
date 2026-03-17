---@meta

---@alias FetchState "bool" | "vec"

---@class Child
local Child = {}

---@return integer?
function Child:id() end

---@async
---@param len integer
---@return string
---@return integer
function Child:read(len) end

---@async
---@return string branch
---@return integer event
function Child:read_line() end

---@async
---@param options table
---@return string branch
---@return integer event
function Child:read_line_with(options) end

---@async
---@param src string
---@return boolean
---@return Error Io
function Child:write_all(src) end

---@async
---@return boolean
---@return Error Io
function Child:flush() end

---@async
---@return any Nil
---@return Error Io
function Child:wait() end

---@async
---@return any
function Child.wait_with_output() end

---@async
---@return any Nil
---@return Error Io
function Child:try_wait() end

---@return boolean
---@return Error Io
function Child:start_kill() end

---@return ChildStdin?
function Child:take_stdin() end

---@return ChildStdout?
function Child:take_stdout() end

---@return ChildStderr?
function Child:take_stderr() end

---@class Command
local Command = {}

---@param arg any
---@return any
function Command.arg(arg) end

---@param dir string
---@return Command
function Command.cwd(dir) end

---@param key string
---@param value string
---@return Command
function Command.env(key, value) end

---@param stdio any
---@return Command
function Command.stdin(stdio) end

---@param stdio any
---@return Command
function Command.stdout(stdio) end

---@param stdio any
---@return Command
function Command.stderr(stdio) end

---@param max integer
---@return Command
function Command.memory(max) end

---@return any Nil
---@return Error Io
function Command:spawn() end

---@async
---@return any Nil
---@return Error Io
function Command:output() end

---@async
---@return any Nil
---@return Error Io
function Command:status() end

---@class Output
---@field status Status (readonly)
---@field stdout string (readonly)
---@field stderr string (readonly)
local Output = {}

---@class Status
---@field success boolean (readonly)
---@field code integer? (readonly)
local Status = {}

---@class Fetcher
---@field cmd string (readonly)
local Fetcher = {}

---@class Spotter
---@field cmd string (readonly)
local Spotter = {}

---@class Preloader
---@field cmd string (readonly)
local Preloader = {}

---@class Previewer
---@field cmd string (readonly)
local Previewer = {}
