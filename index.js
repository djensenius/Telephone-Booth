var express = require('express'),
app = express(),
config = require('./config'),
server = require('http').createServer(app),
mic = require('microphone'),
fs = require('fs'),
request = require('request'),
mpg321 = require('mpg321');

app.set('port', config.portNum + 1);

var hookOn = true;
var recording = false;
var listening = false;

var stream;

const gpio = require('rpi-gpio');
var pRotary = false;
var pPulse = false;
var pulse = 1;

gpio.on('change', function(channel, value) {
    if (channel == 11) { // Rotary Channel
        if (value == true
            && pRotary == false) { // Rotary Start
            pRotary = true;
        } else if (value == false
            && pRotary == true) { // Rotary End
            pRotary = false;

            var digit = pulse - 1;
            if (digit == 10) { // Handle 0
                digit = 0;
            } else if (digit > 10) { // Invalid Digit
                digit = -1;
            }

            if (digit != -1) { // Report back all but invalid digits
                console.log('--- ROTARY REPORTED PULSE', digit);
            }

            pulse = 0; // Reset Pulse
        }
    } else if (channel == 13) { // Pulse
        if (value == true
            && pPulse == false) { // Step Count
            pPulse = true;
        } else if (value == false
            && pPulse == true) { // End step, ignore extra values
            pPulse = false;
            pulse ++;
        }
    } else if (channel == 15) {
	console.log("Hangupper ", value);
    }
    console.log(channel, value);
});

gpio.setup(11, gpio.DIR_IN, gpio.EDGE_BOTH);
gpio.setup(13, gpio.DIR_IN, gpio.EDGE_BOTH);
gpio.setup(15, gpio.DIR_IN, gpio.EDGE_BOTH);

function startRecording() {
	stream = fs.createWriteStream(config.answerPath + "/" + Date.now() + ".mp3");
	mic.startCapture({'mp3output' : true});
	recording = true;
	//Send note that recording has started
}

mic.audioStream.on('data', function(data) {
        stream.write(data);
});

function stopRecording() {
	mic.stopCapture();
	stream.end();

	//Upload file to server to process
}

function getMessage() {

}

function getQuestion() {
	console.log("Getting question");
	var url = config.apiRoot + '/randomQuestion';

	request.get({
		'auth' : {
			'user' : config.login,
			'pass' : config.password,
			'sendImmediately' : false
		},
		'url': url,
		'json': true
	}, function (error, response, body) {
		console.log("Done...");
    	if (!error && response.statusCode === 200) {
        	console.log(body) // Print the json response
			var extension = body.file.title.split('.').pop();
			let file = config.questionPath + body.file._id + "." + extension;

			fs.stat(file, function(err, stat) {
				if(err == null) {
					console.log('File exists');
					playFile(file, "question");
				} else if(err.code == 'ENOENT') {
					// file does not exist
					console.log("Gotta download the file ", file);
					let downloadUrl = config.apiRoot + '/download/question/' + body.file._id  + "." + extension;
					console.log("Grabbing ", downloadUrl);
					request.get({
						'auth' : {
							'user' : config.login,
							'pass' : config.password,
							'sendImmediately' : false
						},
						'url': downloadUrl
					}, function(errorFile, responseFile, bodyFile) {
						console.log("Done downloading, should play");
						playFile(file, "question");
					}).pipe(fs.createWriteStream(file));
				} else {
					console.log('Some other error: ', err.code);
				}
			});
    	}
	});
}

function playFile(file, type) {
	//Play file, once done start recording
	//If type == question, begin recording after
	//If type == answer, set back to dialtone
	/*
	console.log("Going to play " + file);
	player = mpg321().remote();
	player.play(file);
	player.on('end', function() {
		console.log("Done playing file");
	});
	*/
}

getQuestion();

server.listen(app.get('port'), function(){
	console.log('Express server listening on port ' + app.get('port'));
});
