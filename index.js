/*
Telephone Booth - David Jensenius
*/

var cluster = require('cluster'),
	numCPUs = require('os').cpus().length;

if (cluster.isMaster) {
  // Fork workers.
  for (var i = 0; i < numCPUs; i++) {
    cluster.fork();
  }

  cluster.on('exit', function(worker, code, signal) {
    console.log('worker ' + worker.process.pid + ' died');
	cluster.fork();
  });
} else {
	// Workers can share any TCP connection
	var express = require('express'),
	app = express(),
	mongoose = require('mongoose'),
	models = require('./models/messages')
	config = require('./config'),
	cluster = require('cluster'),
	morgan = require('morgan'),
	server = require('http').createServer(app);

	var ObjectId = require('mongoose').Types.ObjectId; //required to query raw objectId in mongoose
	var mongoConnect = mongoose.connect(config.mongooseAuth);

	app.use(morgan('dev')); // log every request to the console
	app.set('port', config.portNum);

	// Handle Errors gracefully
	app.use(function(err, req, res, next) {
		if(!err) return next();
		console.log(err.stack);
		res.json({error: true});
	});

	server.listen(app.get('port'), function(){
		console.log('Express server listening on port ' + app.get('port'));
	});

	// Phone code
	// Taken from piphone - https://github.com/steven-gardiner/piphone
	var piphone = {};
	piphone.mods = {};
	piphone.mods.cp = require('child_process');
	piphone.mods.gpiobutton = require('gpiobutton');
	piphone.mods.fs = require('fs');

	piphone.digits = {
		'1': '.*',
	  	'2': '[abc]',
	  	'3': '[def]',
	  	'4': '[ghi]',
	  	'5': '[jkl]',
	  	'6': '[mno]',
	  	'7': '[pqrs]',
	  	'8': '[tuv]',
	  	'9': '[wxyz]',
	  	'0': ' '
	};

  	piphone.state = {};
  	piphone.state.mode = "";
	piphone.state.sofar = [];

	piphone.dev = {};
	piphone.dev.hook = new piphone.mods.gpiobutton.button({name:'hook', gpiono:22, DOWN:1, interval:20});
	piphone.dev.dial = new piphone.mods.gpiobutton.button({name:'dial', gpiono:27, longTimeout: 10000});
	piphone.dev.rotary = new piphone.mods.gpiobutton.button({name:'rotary', gpiono:17, interval:20, DOWN:1});
	//piphone.dev.onoff = new piphone.mods.gpiobutton.button({name:'switch', gpiono: 18});

	piphone.dev.hook.on('buttondown', function() {
		process.emit('clear_code');
		if (piphone.mike) {
			piphone.mike.kill();
		}
	});

	piphone.dev.hook.on('longpress', function() {
  		process.emit('mpc', {cmd:['pause']});
	});

	piphone.dev.hook.on('buttonpress', function() {
  		process.emit("volume", {volume:100});
	});

	piphone.dev.hook.on('multipress', function(spec) {
  		var vol = 120 - (10*(spec.count));
  		process.emit("volume", {volume:vol});
	});

	piphone.dev.dial.on('longpress', function() {
		console.log("LONG!");
		process.emit('setmode', {from:piphone.state.mode, to:'-'});
	});

	piphone.dev.rotary.on('multipress', function(spec) {
		var digit = Math.ceil(spec.count) % 10;
		//piphone.state.sofar.push(piphone.state.mode);
		piphone.state.sofar.push(digit);
		process.emit('code');

		switch (piphone.state.mode) {
			case '-':
				process.emit('setmode', {from:piphone.state.mode, to:''});
				break;
			case '*':
				if (piphone.state.sofar.length >= 3) {
					process.emit('setmode', {from:piphone.state.mode, to:''});
					process.emit('clear_code');
				}
      			break;
			default:
		}
	});

	piphone.dev.rotary.on('buttonpress', function(spec) {
		piphone.dev.rotary.emit('multipress', spec);
	});

	process.on('code', function(spec) {
		var code = piphone.state.sofar.join("");
		console.log("CODE %j", {code:code, state:piphone.state});

		if (piphone.state.sofar[0] === 0) {
			process.emit('rotary_query', {rquery:piphone.state.sofar.slice(1)});
			return;
		}

		switch (code) {
    		case '1':
      			process.emit("mpc", {cmd:'play'});
      			piphone.state.sofar.shift();
      			break;
    		case '2':
      			process.emit("tts", {text:['number','2']});
      			process.emit("mpcq", {query:['little','bird,','little','bird']});
      			piphone.state.sofar.shift();
      			break;
    		case '3':
		      	process.emit("tts", {text:['number','3']});
		      	process.emit("mpcq", {query:['humpty','dumpty']});
		      	piphone.state.sofar.shift();
		      	break;
		    case '4':
		      	process.emit("tts", {text:['number','4']});
		      	process.emit("mpcq", {query:['wiggle','tooth']});
		      	piphone.state.sofar.shift();
		      	break;
		    case '5':
		      	process.emit("tts", {text:['number','5']});
		      	process.emit("mpcq", {query:['guapo']});
		      	piphone.state.sofar.shift();
		      	break;
		    case '6':
		      	process.emit("tts", {text:['number','6']});
		      	process.emit("mpcq", {query:['belafonte','matilda']});
		      	piphone.state.sofar.shift();
		      	break;
		    case '7':
		      	process.emit("tts", {text:['number','7']});
		      	process.emit("mpcq", {query:['susanna','tanyas']});
		      	piphone.state.sofar.shift();
		      	break;
		    case '8':
		      	process.emit("tts", {text:['number','8']});
		      	process.emit("mpcq", {query:['puff']});
		      	piphone.state.sofar.shift();
		      	break;
		    case '9':
		      	process.emit("tts", {text:['number','9']});
		      	process.emit("mpcq", {query:['lilly']});
		      	piphone.state.sofar.shift();
		      	break;
		    case '-1':
		      	process.emit('clear_code');
		      	process.emit('setmode', {from:piphone.state.mode, to:'*'});
		      	piphone.state.sofar.push("*");
		      	break;
		    case '-2':
				process.emit('clear_code');
		      	process.emit('setmode', {from:piphone.state.mode, to:'#'});
		      	piphone.state.sofar.push("#");
		      	break;
		    case '-3':
		    case '-4':
		    case '-5':
		    case '-6':
		    case '-7':
		    case '-8':
		    case '-9':
		      	process.emit('mpc', {cmd:['pause']});
		      	process.emit('mike', {id:piphone.state.sofar.pop()});
		      	process.emit('clear_code');
		      	break;
		    case '-0':
		      	process.emit('setmode', {from:piphone.state.mode, to:''});
		      	process.emit('clear_code');
		      	process.emit('clear_recs');
		      	break;
		    case "*60":
		      	process.emit('mpc', {cmd:['single', 'on']});
		      	process.emit('audible_status');
		      	break;
		    case "*80":
		      	process.emit('mpc', {cmd:['single', 'off']});
		      	process.emit('audible_status');
		      	break;
		    case "*61":
		      	process.emit('mpc', {cmd:['random', 'on']});
		      	process.emit('audible_status');
		      	break;
		    case "*81":
		      	process.emit('mpc', {cmd:['random', 'off']});
		      	process.emit('audible_status');
		      	break;
		    case "*65":
		      	process.emit('audible_trackid');
		      	break;
		    case "*66":
		      	process.emit('mpc', {cmd:['repeat', 'on']});
		      	process.emit('audible_status');
		      	break;
		    case "*86":
		      	process.emit('mpc', {cmd:['repeat', 'off']});
		      	process.emit('audible_status');
		      	break;
		    case '*78':
		      	process.emit('shutdown_request');
		      	break;
			}
		});

		process.on('rotary_query', function(spec) {
  			if (spec.rquery.length === 0) {
    			return;
  			}

  			spec.regex = spec.rquery.map(function(digit) { return piphone.digits[digit]; }).join('');
  			console.error("RQUERY %j", spec);

  			var mpcq = piphone.mods.cp.exec(['mpc_query', spec.regex].join(' '), function(code, out, err) {

    		if (out.length === 0) {
      			process.emit('effect', {name:'beep'});
      			return;
    		}

    		var lines = out.split(/\n/);
    		console.error("MPCQ %j", {code:code,out:out.slice(0,100),err:err,lines:lines.slice(0,10)});
    		if (lines.length === 2) {
      			process.emit('mpc', {cmd:['play',lines[0]]});
      			return;
    		}
    		//process.emit('effect', {name:'uhoh'});
    		//process.emit('tts', {text:[lines.length]});
		});
	});

	process.on('clear_code', function(spec) {
  		piphone.state.sofar = [];
	});


	var saveFile = function() {

	};

	var syncServer = function() {

	};
}

/*
Notes:
Save audio recordings in mongodatabase.
Once an audio recording is saved, try to synchronize to web server
Periodically also try to sync with web server
Associate recorded sounds with questions asked, for reference
*/
