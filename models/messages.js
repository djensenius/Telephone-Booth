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
		question: String,
		status: String,
		file: File,
		playCount: Number
    });
	Message.plugin(timestamps);

	Status = new Schema({
		ping: Date,
		hookOn: Boolean,
		recording: Boolean,
		listeningQuestion: Boolean,
		listeningMessage: Boolean,
		messagePlays: Number,
		questionPlays: Number
	});
	Status.plugin(timestamps);

	 mongoose.model('Message', Message);
	 mongoose.model('Question', Question);
	 mongoose.model('File', File);
	 mongoose.model('Status', Status);
	 fn()
}

exports.defineModels = defineModels;
