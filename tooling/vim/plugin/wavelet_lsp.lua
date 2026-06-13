-- Start the Wavelet language server (wavelet-lsp) for `.wvl` buffers in Neovim.
--
-- Neovim only: classic Vim has no built-in LSP client, so this is a no-op there
-- and you still get syntax highlighting from syntax/wavelet.vim.
--
-- The server is located, in order:
--   1. g:wavelet_lsp_path, if set;
--   2. a binary bundled in this package's `bin/` dir matching the platform
--      (release zips ship one per platform here);
--   3. `wavelet-lsp` on the PATH.
-- If none is found, this is a silent no-op.

if vim.fn.has("nvim-0.8") == 0 then
  return
end

local function bundled_server()
  local uname = vim.loop.os_uname()
  local triples = {
    ["Darwin:arm64"] = "aarch64-apple-darwin",
    ["Darwin:x86_64"] = "x86_64-apple-darwin",
    ["Linux:x86_64"] = "x86_64-unknown-linux-gnu",
    ["Windows_NT:x86_64"] = "x86_64-pc-windows-msvc",
  }
  local triple = triples[uname.sysname .. ":" .. uname.machine]
  if not triple then
    return nil
  end
  local exe = uname.sysname == "Windows_NT" and ".exe" or ""
  -- This file is <pkg>/plugin/wavelet_lsp.lua; the binary is <pkg>/bin/...
  local pkg = vim.fn.fnamemodify(vim.fn.resolve(debug.getinfo(1, "S").source:sub(2)), ":p:h:h")
  local candidate = pkg .. "/bin/wavelet-lsp-" .. triple .. exe
  if vim.fn.filereadable(candidate) == 1 then
    return candidate
  end
  return nil
end

local function server_cmd()
  if vim.g.wavelet_lsp_path and vim.g.wavelet_lsp_path ~= "" then
    return vim.g.wavelet_lsp_path
  end
  local bundled = bundled_server()
  if bundled then
    return bundled
  end
  local on_path = vim.fn.exepath("wavelet-lsp")
  if on_path ~= "" then
    return on_path
  end
  return nil
end

vim.api.nvim_create_autocmd("FileType", {
  pattern = "wavelet",
  callback = function(args)
    local cmd = server_cmd()
    if not cmd then
      return
    end
    vim.lsp.start({
      name = "wavelet-lsp",
      cmd = { cmd },
      root_dir = vim.fs.dirname(vim.api.nvim_buf_get_name(args.buf)),
    })
  end,
})
