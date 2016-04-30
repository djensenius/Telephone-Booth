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
}

/*
Notes:
Save audio recordings in mongodatabase.
Once an audio recording is saved, try to synchronize to web server
Periodically also try to sync with web server
Associate recorded sounds with questions asked, for reference
*/
