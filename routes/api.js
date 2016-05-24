module.exports = function(app, multipartyMiddleware) {
    app.post('/message', multipartyMiddleware, function(req, res, next) {
        var file = req.files.file;
        var uploadedFile = new Files({title: file.name});
        var extension = uploadedFile.title.split('.').pop();
		var tmp_path = file.path;
		var target_path = config.uploadPath + uploadedFile._id;
        var sym_path = target_path + "." + extension;

        let question = req.question;

        var message = new Message({question: question, status: "Pending"});
        message.file = uploadedFile;
		console.log('Target path is: ' + target_path);
		// move the file from the temporary location to the intended location
		fs.rename(tmp_path, target_path, function(err) {
			if (err) throw err;
		    // delete the temporary file, so that the explicitly set temporary upload dir does not get filled with unwanted files
			fs.unlink(tmp_path, function() {
                fs.symlink(target_path, sym_path, function(err) {
                    if (err) throw err;
                });
				if (err) throw err;
				message.save(function(err) {
					if(err) {
						//req.flash('error', 'There was a problem updating your post. Please try again later');
						console.log('error updating ' + err);
					} else {
						console.log("Success!!!");
						res.send('File uploaded to: ' + target_path + ' - ' + req.files.size + ' bytes');
					}
				});
			});
		});
    });

    app.post('/question', multipartyMiddleware, function(req, res, next) {

    });

    app.get('/questions', function(req,res) {
		res.render('index');
	});
};
