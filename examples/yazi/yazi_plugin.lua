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
---@return any
function Child:read_line() end

---@async
---@param options table
---@return any
function Child:read_line_with(options) end

---@async
---@param src string
---@return any
function Child:write_all(src) end

---@async
---@return any
function Child:flush() end

---@async
---@return any
function Child:wait() end

---@async
---@param ud any
---@return Child
function Child.wait_with_output(ud) end

---@async
---@return any
function Child:try_wait() end

---@return any
function Child:start_kill() end

---@return any
function Child:take_stdin() end

---@return any
function Child:take_stdout() end

---@return any
function Child:take_stderr() end

---@class Command
local Command = {}

---@param arg any
---@return Command
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

---@return any
function Command:spawn() end

---@async
---@return any
function Command:output() end

---@async
---@return any
function Command:status() end

---@class Output
---@field status any (readonly)
---@field stdout any (readonly)
---@field stderr any (readonly)
local Output = {}

---@class Status
---@field success boolean (readonly)
---@field code integer? (readonly)
local Status = {}

---@class Fetcher
---@field cmd any (readonly)
local Fetcher = {}

---@class Spotter
---@field cmd any (readonly)
local Spotter = {}

---@class Preloader
---@field cmd any (readonly)
local Preloader = {}

---@class Previewer
---@field cmd any (readonly)
local Previewer = {}
