var express = require('express'),
	app = express(),
	config = require('./config'),
	server = require('http').createServer(app),
	mic = require('microphone'),
	fs = require('fs'),
	request = require('request'),
	_ = require('lodash'),
	mpg321 = require('mpg321');

app.set('port', config.portNum + 1);

var hookOn = true;
var recording = false;
var listeningQuestion = false;
var listeningMessage = false;
var playingDialTone = false;
var listeningInstructions = false;
var recordingFile = "";
var recordingFileName = "";
var playingType = "tone";

var currentQuestion;
var stream;

player = mpg321().remote();
player.on('end', function() {
	console.log("Done playing file");
	player.stop();
	if (playingType == "question") {
		listeningQuestion = false;
		playingType = "beep";
		playFile('Beep.mp3');
		setTimeout(function() {
			startRecording();
		}, 1700);
		var formData = {
			// Pass date
			status: "listeningQuestion",
			value: false
		}
		updateStatus(formData);
	} else if (playingType == "message") {
		listeningMessage = false;
		var formData = {
			// Pass date
			status: "listeningMessage",
			value: false
		}
		updateStatus(formData);
		player.play('DialTone.mp3');
		playingDialTone = true;
		playingType = "tone";
	} else if (playingType == "instructions") {
		listeningInstructions = false;
		var formData = {
			// Pass date
			status: "listeningInstructions",
			value: false
		}
		updateStatus(formData);
		player.play('DialTone.mp3');
		playingDialTone = true;
		playingType = "tone";
	} else if (playingType == "tone") {
		playFile('DialTone.mp3');
	}
});

if (config.enablegpio == true) {
	const gpio = require('rpi-gpio');
	var pRotary = false;
	var pPulse = false;
	var pulse = 1;

	gpio.on('change', function(channel, value) {
		if (channel == 11) { // Rotary Channel
			if (value == true && pRotary == false) { // Rotary Start
				pRotary = true;
			} else if (value == false && pRotary == true) { // Rotary End
				pRotary = false;

				var digit = pulse - 1;
				if (digit == 10) { // Handle 0
					digit = 0;
				} else if (digit > 10) { // Invalid Digit
					digit = -1;
				}

				if (digit != -1) { // Report back all but invalid digits
					console.log('--- ROTARY REPORTED PULSE', digit);
					// TODO: Eliminate duplicate actiotns
					if (hookOn == false) {
						if (digit == 1) {
							getQuestion();
						} else if (digit == 2) {
							getMessage();
						} else if (digit > 2) {
							playInstructions();
						}
					}
				}
				pulse = 0; // Reset Pulse
			}
		} else if (channel == 13) { // Pulse
			if (value == true && pPulse == false) { // Step Count
				pPulse = true;
			} else if (value == false && pPulse == true) { // End step, ignore extra values
				pPulse = false;
				pulse ++;
			}
		} else if (channel == 15) {
			if (value == true) {
				hangUp();
			} else {
				pickUp();
			}
			console.log("Hangupper ", value);
		}
		console.log(channel, value);
	});

	gpio.setup(11, gpio.DIR_IN, gpio.EDGE_BOTH);
	gpio.setup(13, gpio.DIR_IN, gpio.EDGE_BOTH);
	gpio.setup(15, gpio.DIR_IN, gpio.EDGE_BOTH);
}

function pickUp() {
	console.log("Playing ringtone");
	player.play('DialTone.mp3');
	playingDialTone = true;
	playingType = "tone";
	hookOn = false;
}

function hangUp() {
	console.log("Hanging up...");
	hookOn = true;
	playingType = "none";
	if (recording == true) {
		stopRecording();
	}

	if (listeningMessage == true) {
		//Stop listening to the message
		listeningMessage = false;
		player.stop();
		var formData = {
			status: "listeningMessage",
			value: false
		}
		updateStatus(formData);
	}

	if (listeningQuestion == true) {
		//Stop listening to the question
		listeningQuestion = false;
		player.stop();
		var formData = {
			status: "listeningQuestion",
			value: false
		}
		updateStatus(formData);
	}

	if (listeningInstructions == true) {
		//Stop listening to the question
		listeningInstructions = false;
		player.stop();
		var formData = {
			status: "listeningInstructions",
			value: false
		}
		updateStatus(formData);
	}

	if (playingDialTone == true) {
		//Stop the dial tone
		playingDialTone = false;
		player.stop();
	}
}

function startRecording() {
	console.log("Going to start recording");
	recordingFileName = String(Date.now()) + ".mp3";
	recordingFile = config.answerPath + recordingFileName;
	stream = fs.createWriteStream(recordingFile);
	mic.startCapture({'mp3output' : true});
	recording = true;
	var formData = {
		// Pass date
		status: "recording",
		value: true
	}
	updateStatus(formData);

	//Send note that recording has started
}

mic.audioStream.on('data', function(data) {
	stream.write(data);
});

function stopRecording() {
	console.log("Going to stop recording " + recordingFileName);
	mic.stopCapture();
	recording = false;
	setTimeout(function() {
		stream.end();
		var formData = {
			// Pass date
			status: "recording",
			value: false
		}
		updateStatus(formData);
		//Upload file to server to process
		var formData = {
			// Pass a simple key-value pair
			question: currentQuestion,
			// Pass data via Streams
			file: {
				value:  fs.createReadStream(recordingFile),
				options: {
					filename: recordingFileName,
					contentType: 'audio/mp3'

				}
			}
		}

		let postURL = config.apiRoot + '/upload/message';
		request.post({
			'auth' : {
				'user' : config.login,
				'pass' : config.password,
				'sendImmediately' : true
			},
			url: postURL,
			formData: formData
		}, function optionalCallback(err, httpResponse, body) {
			if (err) {
				return console.error('upload failed:', err);
			}
			console.log('Upload successful!  Server responded with:', body);
		});
	}, 2000);
}

function getMessage() {
	console.log("Getting message");
	listeningMessage = true;
	var formData = {
		// Pass date
		status: "listeningMessage",
		value: true
	}
	updateStatus(formData);
	var url = config.apiRoot + '/randomMessage';

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
			var extension = body.file.title.split('.').pop();
			let file = config.answerPath + body.file._id + "." + extension;

			fs.stat(file, function(err, stat) {
				if(err == null) {
					console.log('File exists');
					playingType = "message";
					playFile(file);
				} else if(err.code == 'ENOENT') {
					// file does not exist
					console.log("Gotta download the file ", file);
					let downloadUrl = config.apiRoot + '/download/message/' + body.file._id  + "." + extension;
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
						playingType = "message";
						playFile(file);
					}).pipe(fs.createWriteStream(file));
				} else {
					console.log('Some other error: ', err.code);
				}
			});
		}
	});
}

function getQuestion() {
	listeningQuestion = true;
	var formData = {
		// Pass date
		status: "listeningQuestion",
		value: true
	}
	updateStatus(formData);
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
			currentQuestion = body._id;
			console.log(body) // Print the json response
			var extension = body.file.title.split('.').pop();
			let file = config.questionPath + body.file._id + "." + extension;

			fs.stat(file, function(err, stat) {
				if(err == null) {
					console.log('File exists');
					playingType = "question";
					playFile(file);
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
						playingType = "question";
						playFile(file, "question");
					}).pipe(fs.createWriteStream(file));
				} else {
					console.log('Some other error: ', err.code);
				}
			});
		}
	});
}

function playInstructions() {
	listeningInstructions = true;
	var formData = {
		// Pass date
		status: "listeningInstructions",
		value: true
	}
	updateStatus(formData);
	console.log("Done...");
	let file = 	'Instructions.mp3';
	playingType = "instructions";
	playFile(file);
}

function playFile(file) {
	//Play file, once done start recording
	//If type == question, begin recording after
	//If type == answer, set back to dialtone
	playingDialTone = false;
	player.stop();
	console.log("Going to play " + file);
	player.play(file);
}

function updateStatus(formData) {
	let putURL = config.apiRoot + '/status';
	request.put({
		'auth' : {
			'user' : config.login,
			'pass' : config.password,
			'sendImmediately' : true
		},
		url: putURL,
		json: formData
	}, function optionalCallback(err, httpResponse, body) {
		if (err) {
			return console.error('upload failed:', err);
		}
	});
}

setInterval(function() {
	var formData = {
		// Pass date
		status: "ping",
		value: Date.now()
	}
	updateStatus(formData);
}, 5000);

server.listen(app.get('port'), function(){
	console.log('Express server listening on port ' + app.get('port'));
});

//HACKY thing to get mpg321 to quit
process.on('SIGINT', function () {
	process.exit();
});
