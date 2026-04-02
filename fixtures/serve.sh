#!/bin/bash
# Serve fixtures on localhost:8787
cd "$(dirname "$0")"
echo "Serving fixtures at http://localhost:8787"
python3 -m http.server 8787
