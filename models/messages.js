function defineModels(mongoose, fn) {
	var Schema = mongoose.Schema,
	ObjectId = Schema.ObjectId;

/* Use embedded model: http://docs.mongodb.org/manual/core/data-model-design/ */

	FileSchema = new Schema ({
		title: String
	});
	FileSchema.plugin(timestamps);

	QuestionSchema = new Schema ({
		description: String,
		voice: String,
		file: File
	});
	QuestionSchema.plugin(timestamps);

    MessageSchema = new Schema ({
		question: Question,
		status: String,
		file: File
    });
	MessageSchema.plugin(timestamps);

	 mongoose.model('Message', Message);
	 mongoose.model('Question', Question);
	 mongoose.model('File', File);
	 fn()
}

exports.defineModels = defineModels;
