#!/bin/bash

RESTART="sudo killall mpg321; cd /home/pi/Code/phone-booth; sudo nohup node index.js > ~/phone.log"

PGREP="pgrep"

NODE="node"

# find httpd pid
$PGREP ${NODE}

if [ $? -ne 0 ] # if node not running
then
 # restart service
 $RESTART
fi
