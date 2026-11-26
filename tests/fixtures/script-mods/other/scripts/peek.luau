-- Another mod, depending on `scripted`, looking for its globals.
function on_peek()
	return secret, on_answer
end

function on_require(): number
	return require("scripted:util").double(21)
end

function on_write_global()
	_G.secret = "theirs"
end

-- What the sandbox leaves out.
function on_escape()
	return type(os), type(io), type(debug), type(loadstring), type(getfenv), type(print), type(collectgarbage)
end
