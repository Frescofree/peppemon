#!/bin/bash
# Launch peppemon in a dedicated terminal window
# Usage: ./launch_peppemon.sh   or   bash launch_peppemon.sh

exec gnome-terminal --title="peppemon" --geometry=130x42 -- /usr/local/bin/peppemon
