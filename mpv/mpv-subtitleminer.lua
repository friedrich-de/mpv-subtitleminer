local utils = require("mp.utils")
local options = require("mp.options")

local opts = {
  -- Either adjust settings here OR in script-opts/mpv-subtitleminer.conf
  -- ========== SETTINGS ==========
  -- List of ports to try starting the server on.
  ports = { 61777, 61778, 61779, 61780, 61781 },
  -- When true (default), starts the mpv-subtitleminer server automatically on mpv startup.
  auto_start = true,
  -- ==============================
}

options.read_options(opts, "mpv-subtitleminer")

-- Convert ports from string to table if needed
if type(opts.ports) == "string" then
  local ports_table = {}
  for port_str in opts.ports:gmatch("[^,]+") do
    local port = tonumber(port_str)
    if port and port > 0 and port <= 65535 then
      table.insert(ports_table, port)
    else
      mp.msg.warn("Invalid port value: " .. port_str)
    end
  end
  opts.ports = ports_table
end

if #opts.ports == 0 then
  mp.msg.error("No valid ports configured")
  mp.osd_message("[mpv-subtitleminer] No valid ports configured", 5)
  return
end

local platform = mp.get_property_native("platform")

local config_file_path = mp.find_config_file("mpv.conf")
if not config_file_path then
  mp.osd_message("[mpv-subtitleminer] Could not find mpv.conf", 5)
  mp.msg.error("Could not find mpv.conf")
  return
end

local config_folder_path = utils.split_path(config_file_path)
local binary_name = platform == "windows" and "mpv-subtitleminer.exe" or "mpv-subtitleminer"
local binary_path = utils.join_path(config_folder_path, binary_name)

-- Verify binary exists
local info, err = utils.file_info(binary_path)
if err then
  mp.osd_message("[mpv-subtitleminer] mpv-subtitleminer binary not found at " .. binary_path, 5)
  mp.msg.error("mpv-subtitleminer binary not found at: " .. binary_path)
  return
else
  mp.msg.info("mpv-subtitleminer binary found at: " .. binary_path)
end

-- Find ffmpeg binary (next to mpv-subtitleminer first, then PATH)
local function find_ffmpeg()
  local ffmpeg_name = platform == "windows" and "ffmpeg.exe" or "ffmpeg"

  -- 1. Check next to mpv-subtitleminer binary
  local local_ffmpeg = utils.join_path(config_folder_path, ffmpeg_name)
  local local_info, local_err = utils.file_info(local_ffmpeg)
  if not local_err then
    mp.msg.info("Found ffmpeg next to binary: " .. local_ffmpeg)
    return local_ffmpeg
  end

  -- 2. Fall back to PATH
  mp.msg.info("Using ffmpeg from PATH")
  return ffmpeg_name
end

-- Check if ffmpeg has https protocol support
local function check_ffmpeg_https(ffmpeg_path)
  local result = mp.command_native({
    name = "subprocess",
    playback_only = false,
    capture_stdout = true,
    capture_stderr = true,
    args = { ffmpeg_path, "-protocols" },
  })

  if result and result.stdout then
    if result.stdout:find("https") then
      mp.msg.info("ffmpeg has HTTPS protocol support")
      return true
    end
  end

  mp.msg.warn("ffmpeg does NOT have HTTPS support - network streams will not work!")
  mp.msg.warn("To enable network support, use an ffmpeg build with openssl/gnutls enabled")
  return false
end

local ffmpeg_path = find_ffmpeg()
local ffmpeg_has_https = check_ffmpeg_https(ffmpeg_path)

local function strip_mpv_conf_value(v)
  if not v then
    return v
  end
  -- Strip leading %<digits>% if present.
  v = v:gsub("^%%(%d+)%%", "")
  return v
end

-- Find the IPC socket path from mpv.conf
local function find_mpv_socket()
  local file = io.open(config_file_path, "r")
  if not file then
    mp.osd_message("Failed to read mpv.conf.", 5)
    mp.msg.error("Failed to read mpv.conf")
    return nil
  end

  local socket_path
  for line in file:lines() do
    socket_path = line:match("^input%-ipc%-server%s*=%s*(.+)$")
    if socket_path then
      -- Trim trailing whitespace and strip prefix
      socket_path = strip_mpv_conf_value(socket_path:gsub("%s+$", ""))
      break
    end
  end
  file:close()

  if not socket_path then
    mp.msg.error("input-ipc-server not configured in mpv.conf")
    mp.osd_message("[mpv-subtitleminer] input-ipc-server not configured in mpv.conf", 5)
    return nil
  end

  if platform == "windows" then
    if not socket_path:match("^\\\\.\\pipe") and not socket_path:match("^//%.%/pipe") then
      socket_path = "\\\\.\\pipe" .. socket_path:gsub("/", "\\")
    else
      socket_path = socket_path:gsub("/", "\\")
    end
  end

  return socket_path
end

local mpv_socket = find_mpv_socket()
if not mpv_socket then
  mp.msg.error("Could not determine mpv IPC socket path")
  mp.osd_message("[mpv-subtitleminer] Could not determine mpv IPC socket path", 5)
  return
else
  mp.msg.info("Using mpv IPC socket at: " .. mpv_socket)
end

local server_process = nil
local server_running = false
local current_port = nil
local startup_timer = nil

local function get_mpv_pid()
  local pid = mp.get_property_native("pid") or mp.get_property_native("process-id")
  if type(pid) == "number" and pid > 0 then
    return pid
  end
  return nil
end

local function try_start_on_port(port_index)
  if port_index > #opts.ports then
    mp.msg.error("Failed to start server on any port")
    mp.osd_message("[mpv-subtitleminer] Failed to start server on any port", 5)
    return
  end

  local port = opts.ports[port_index]
  mp.msg.info("Trying to start server on port " .. port .. "...")
  current_port = port

  local args = { binary_path, mpv_socket, tostring(port), ffmpeg_path }
  local mpv_pid = get_mpv_pid()
  if mpv_pid then
    table.insert(args, "--expected-mpv-pid")
    table.insert(args, tostring(mpv_pid))
  else
    mp.msg.warn("Could not determine mpv PID; instance validation disabled")
  end

  server_process = mp.command_native_async({
    name = "subprocess",
    playback_only = false,
    capture_stdout = true,
    capture_stderr = true,
    args = args,
  }, function(success, result, error)
    local was_confirmed_running = server_running
    server_running = false
    server_process = nil

    if startup_timer then
      startup_timer:kill()
      startup_timer = nil
    end

    local stderr_text = result and result.stderr or ""
    local is_port_error = stderr_text:find("Address already in use") or
        stderr_text:find("os error 98") or
        stderr_text:find("os error 10048") -- Windows

    if result then
      if result.stdout and result.stdout ~= "" then
        for line in result.stdout:gmatch("[^\r\n]+") do
          mp.msg.info("" .. line)
        end
      end
      if result.stderr and result.stderr ~= "" then
        for line in result.stderr:gmatch("[^\r\n]+") do
          mp.msg.warn("" .. line)
        end
      end
    end

    if stderr_text:find("MPV_IPC_PID_MISMATCH") then
      mp.msg.error("mpv IPC socket belongs to a different mpv instance (shared input-ipc-server)")
      mp.osd_message(
        "[mpv-subtitleminer]: IPC socket is in use by another mpv instance\nSet a unique input-ipc-server per instance",
        6)
      current_port = nil
      return
    end

    if stderr_text:find("Failed to connect to mpv socket") or stderr_text:find("Failed to connect to mpv pipe") then
      mp.msg.error("Could not connect to mpv IPC socket at: " .. mpv_socket)
      mp.osd_message("[mpv-subtitleminer]: can't connect to mpv IPC socket\n" .. mpv_socket, 6)
      current_port = nil
      return
    end

    if is_port_error then
      mp.msg.info("Port " .. port .. " in use, trying next...")
      try_start_on_port(port_index + 1)
      return
    end

    if was_confirmed_running then
      if not success then
        local error_msg = error or "unknown error"
        mp.msg.error("Server exited with error: " .. error_msg)
        mp.osd_message("[mpv-subtitleminer] server stopped (error)", 3)
      else
        local status = result and result.status or "unknown"
        mp.msg.info("Server exited with status: " .. tostring(status))
        mp.osd_message("[mpv-subtitleminer] server stopped", 2)
      end
    end

    current_port = nil
  end)

  if server_process then
    -- Use a timer to confirm the server started successfully
    -- If it's still running after 500ms, consider it started
    startup_timer = mp.add_timeout(0.5, function()
      startup_timer = nil
      if server_process then
        server_running = true
        mp.msg.info("Server confirmed running on port " .. port)
        mp.osd_message("[mpv-subtitleminer] server started on port " .. port, 3)
      end
    end)
  else
    mp.msg.error("Failed to start server process")
    mp.osd_message("[mpv-subtitleminer] Failed to start server", 3)
  end
end

local function start_server()
  if server_running or server_process then
    mp.osd_message("[mpv-subtitleminer] Server already running on port " .. (current_port or "unknown"), 2)
    return
  end

  mp.msg.info("Starting mpv-subtitleminer server...")
  mp.osd_message("[mpv-subtitleminer] Starting server...", 2)

  try_start_on_port(1)
end

local function stop_server()
  if not server_process then
    mp.osd_message("[mpv-subtitleminer] Server not running", 2)
    return
  end

  mp.msg.info("Stopping mpv-subtitleminer server...")
  mp.osd_message("[mpv-subtitleminer] Stopping server...", 2)

  local proc = server_process
  server_process = nil
  server_running = false
  mp.abort_async_command(proc)
end

local function toggle_server()
  if server_running or server_process then
    stop_server()
  else
    start_server()
  end
end

if opts.auto_start then
  mp.add_timeout(1, function()
    start_server()
  end)
end

mp.register_script_message("toggle-subtitleminer", toggle_server)
mp.add_key_binding("Ctrl+a", "toggle-subtitleminer", toggle_server)

mp.register_event("shutdown", function()
  if startup_timer then
    startup_timer:kill()
    startup_timer = nil
  end
  if server_running and server_process then
    mp.msg.info("Shutting down server on mpv exit...")
    mp.abort_async_command(server_process)
  end
end)
