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
	multiparty = require('connect-multiparty'),
	pug = require('pug'),
	server = require('http').createServer(app),
	io = require('socket.io').listen(server);

	var ObjectId = require('mongoose').Types.ObjectId; //required to query raw objectId in mongoose
	var mongoConnect = mongoose.connect(config.mongooseAuth);
	var multipartyMiddleware = multiparty();

	app.use(morgan('dev')); // log every request to the console
	app.set('port', config.portNum);
	app.set('view engine', 'pug');
	app.use('/components', express.static(__dirname + '/bower_components'));
	app.use('/download', express.static('/Users/david/uploads'));

	// Handle Errors gracefully
	app.use(function(err, req, res, next) {
		if(!err) return next();
		console.log(err.stack);
		res.json({error: true});
	});

	require('./routes/api.js')(app, multipartyMiddleware);

	server.listen(app.get('port'), function(){
		console.log('Express server listening on port ' + app.get('port'));
	});

}
