function defineModels(mongoose, fn) {
	var Schema = mongoose.Schema,
	ObjectId = Schema.ObjectId;

/* Use embedded model: http://docs.mongodb.org/manual/core/data-model-design/ */

	File = new Schema ({
		title: String
	});
	File.plugin(timestamps);

	Question = new Schema ({
		description: String,
		voice: String,
		file: File,
		playCount: Number
	});
	Question.plugin(timestamps);

    Message = new Schema ({
		question: Question,
		status: String,
		file: File,
		playCount: Number
    });
	Message.plugin(timestamps);

	 mongoose.model('Message', Message);
	 mongoose.model('Question', Question);
	 mongoose.model('File', File);
	 fn()
}

exports.defineModels = defineModels;
