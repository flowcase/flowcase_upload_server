import argparse
import base64
import os
from flask import Flask, request, make_response

app = Flask(__name__)

parser = argparse.ArgumentParser()
parser.add_argument("--ssl", action="store_true")
parser.add_argument("--auth-token")
parser.add_argument("--port", default="4902")
parser.add_argument("--upload-dir", default=(os.path.join(os.getenv("HOME"), "Uploads")))
args, _ = parser.parse_known_args()

@app.route("/upload", methods=["POST"])
def upload():
	if args.auth_token != "":
		if "Authorization" in request.headers and request.headers["Authorization"].startswith("Basic "):
			try:
				user_pass_raw = base64.b64decode(request.headers["Authorization"].replace("Basic ", ""))
			except:
				return make_response(('Failed to decode auth 1', 403))
			else:
				try:
					user_pass_as_text = user_pass_raw.decode("ISO-8859-1")
				except:
					return make_response(('Failed to decode auth 2', 403))
				else:
					if user_pass_as_text != args.auth_token:
						return make_response(('Access Denied!', 403))
	else:
		return make_response(('Authorization Header missing or invalid', 403))

	file = request.files["file"]
	current_chunk = int(request.form["dzchunkindex"])
	if args.upload_dir:
		save_path = os.path.join(args.upload_dir, escapeFilename(file.filename))
		if os.path.exists(save_path):
			if current_chunk == 0:
				return make_response(('File already exists', 400))
		save_path = os.path.join(args.upload_dir, "." + escapeFilename(file.filename) + ".uploading")
		if os.path.exists(save_path) and current_chunk == 0:
			os.remove(save_path)
	else:
		return make_response(("Couldn't find upload path", 500))

	if current_chunk == 0:
		syssize = os.statvfs(args.upload_dir)
		space = syssize.f_bsize * syssize.f_bavail
		if space - int(request.form["dztotalfilesize"]) < 0:
			return make_response(('No Space available', 400))
	try:
		with open(save_path, "ab") as f:
			f.seek(int(request.form["dzchunkbyteoffset"]))
			f.write(file.stream.read())
	except OSError:
		return make_response(("Couldn't write the file to disk", 500))
	else:
		total_chunks = int(request.form["dztotalchunkcount"])
		if current_chunk + 1 == total_chunks:
			if os.path.getsize(save_path) != int(request.form["dztotalfilesize"]):
				os.remove(save_path)
				return make_response(('Size mismatch', 500))
			os.rename(save_path, os.path.join(args.upload_dir, escapeFilename(file.filename)))
		return make_response(('uploaded Chunk', 200))

def escapeFilename(filename):
	keepcharacters = (' ', '.', '_', '-')
	return "".join((c for c in filename if c.isalnum() or c in keepcharacters)).rstrip()

if __name__ == "__main__":
	app.run(host="0.0.0.0", port=args.port, ssl_context="adhoc" if args.ssl else None)