var app = angular.module('TelephoneBoothApp', ['ngMaterial', 'ngFileUpload', 'ngAudio']);

app.controller('PhoneBothCtrl', ['$scope', '$mdDialog', '$http', '$rootScope', 'ngAudio', 'socket', function($scope, $mdDialog, $http, $rootScope, ngAudio, socket){

    $scope.questions = [];
    $scope.pendings = [];
    $scope.approved = [];
    $scope.rejected = [];
    $scope.sound = [];

    $scope.addNewQuestion = function(ev) {
        $scope.currentUploadMessageId = 0;
        $scope.fileUploaded = false;

        $mdDialog.show({
            controller: NewQuestionController,
            templateUrl: '/modals/new_question',
            targetEvent: ev
        })
        .then(function(answer) {
            $scope.alert = 'You said the information was "' + answer + '".';
            $http.get('/questions').success(function(response) {
                $scope.questions = response;
            });
        }, function() {
            $scope.alert = 'You cancelled the dialog.';
        });
    };

    $scope.addNewMessage = function(ev) {
        $scope.fileUploaded = false;

        $mdDialog.show({
            controller: NewMessageController,
            templateUrl: '/modals/new_message',
            targetEvent: ev
        })
        .then(function(answer) {
            $scope.alert = 'You said the information was "' + answer + '".';
            $http.get('/questions').success(function(response) {
                $scope.questions = response;
            });
        }, function() {
            $scope.alert = 'You cancelled the dialog.';
        });
    };

    $scope.deleteQuestion = function(id) {
        var deleteUrl = '/question/' + id;
        $http({method: 'DELETE', url: deleteUrl}).success(function(response) {
            //$rootScope.loadLayers(mapid);
            console.log("Deleted sound");
            loadQuestions();
        }).error(function(response) {
            console.log('Did not delete');
        });
    }

    $scope.deleteMessage = function(id) {
        var deleteUrl = '/message/' + id;
        $http({method: 'DELETE', url: deleteUrl}).success(function(response) {
            //$rootScope.loadLayers(mapid);
            console.log("Deleted sound");
            reloadMessages()
        }).error(function(response) {
            console.log('Did not delete');
        });
    }

    $scope.approveMessage = function(id) {
        var approveUrl = '/message/approve/' + id;
        $http({method: 'PUT', url: approveUrl}).success(function(response) {
            //$rootScope.loadLayers(mapid);
            console.log("Approved sound");
            reloadMessages();
        }).error(function(response) {
            console.log('Did not delete');
        });
    }

    $scope.rejectMessage = function(id) {
        var rejectUrl = '/message/reject/' + id;
        $http({method: 'PUT', url: rejectUrl}).success(function(response) {
            //$rootScope.loadLayers(mapid);
            console.log("Rejected sound");
            reloadMessages();
        }).error(function(response) {
            console.log('Did not delete');
        });
    }

    function loadQuestions() {
        $http.get('/questions').success(function(response) {
            for (var i = 0; i < response.length; i++) {
                var sound_id = response[i].file._id;
                var extension = response[i].file.title.split('.').pop();
                $scope.sound[sound_id] = ngAudio.load("/download/question/" + sound_id + "." + extension);
            }
            $scope.questions = response;
        });
    }
    loadQuestions();

    function loadPending() {
        $http.get('/pending').success(function(response) {
            for (var i = 0; i < response.length; i++) {
                var sound_id = response[i].file._id;
                var extension = response[i].file.title.split('.').pop();
                $scope.sound[sound_id] = ngAudio.load("/download/message/" + sound_id + "." + extension);
            }
            $scope.pendings = response;
        });
    }
    loadPending();

    function loadApproved() {
        $http.get('/approved').success(function(response) {
            for (var i = 0; i < response.length; i++) {
                var sound_id = response[i].file._id;
                var extension = response[i].file.title.split('.').pop();
                $scope.sound[sound_id] = ngAudio.load("/download/message/" + sound_id + "." + extension);
            }
            $scope.approved = response;
        });
    }
    loadApproved()

    function loadRejected() {
        $http.get('/rejected').success(function(response) {
            for (var i = 0; i < response.length; i++) {
                var sound_id = response[i].file._id;
                var extension = response[i].file.title.split('.').pop();
                $scope.sound[sound_id] = ngAudio.load("/download/message/" + sound_id + "." + extension);
            }
            $scope.rejected = response;
        });
    }
    loadRejected()

    function reloadMessages() {
        $scope.pendings = [];
        loadPending();
        $scope.approved = [];
        loadApproved();
        $scope.rejected = [];
        loadRejected();
    }

    socket.on('status', function (data) {
		console.log("Status is: ", data);
        $scope.lastSeen = data.ping;
        $scope.listeningMessage = data.listeningMessage;
        $scope.listeningQuestion = data.listeningQuestion;
        $scope.recording = data.recording;
        $scope.hook = data.hook;
	});

    function NewQuestionController($scope, $mdDialog, $http) {
        $rootScope.$watch('currentUploadMessageId', function() {
            $scope.currentUploadMessageId = $rootScope.currentUploadMessageId;
        });

        $rootScope.$watch('fileUploaded', function() {
            $scope.fileUploaded = $rootScope.fileUploaded;
            console.log("File uploaded changed on the root scope!", $scope.fileUploaded);
        });

        $scope.answer = '';
        $scope.hide = function() {
            $mdDialog.hide();
        };

        $scope.cancel = function() {
            console.log('Canceled');
            $mdDialog.cancel();
        };

        $scope.answer = function(type) {
            console.log('New Quetion! ' + $scope.question.description + ' from ' + $scope.question.voice);
            if (type == 'Add') {
                var postData = JSON.stringify({'description': $scope.question.description, 'voice': $scope.question.voice, 'id': $scope.currentUploadMessageId});
                console.log(postData);
                $http({
                    method: 'PUT',
                    url: '/question',
                    data: postData,
                    contentType: 'application/json', // content type sent to server
                    dataType: 'json', //Expected data format from server
                    processdata: true, //True or False
                    crossDomain: true,
                }).success(function(response) {
                    console.log('Whee! ' + response.codeStatus);
                    $mdDialog.hide();
                    loadQuestions();
                }).error(function(response) {
                    console.log("error"); // Getting Error Response in Callback
                    $scope.codeStatus = response || "Request failed";
                    console.log($scope.codeStatus);
                });
            }
        };
    }

    function NewMessageController($scope, $mdDialog, $http) {
        $rootScope.$watch('currentUploadMessageId', function() {
            $scope.currentUploadMessageId = $rootScope.currentUploadMessageId;
        });

        $rootScope.$watch('fileUploaded', function() {
            $scope.fileUploaded = $rootScope.fileUploaded;
            console.log("File uploaded changed on the root scope!", $scope.fileUploaded);
        });

        $scope.answer = '';
        $scope.hide = function() {
            $mdDialog.hide();
        };

        $scope.cancel = function() {
            console.log('Canceled');
            $mdDialog.cancel();
        };

        $scope.answer = function(type) {
            if (type == 'Add') {
                $mdDialog.hide();
            }
        };
    }
}]);

app.controller('eventUpload', ['$scope', '$rootScope', 'Upload', function ($scope, $rootScope, Upload) {
    $scope.mode = 'query';
    $scope.determinateValue = 0;
    $scope.$watch('files', function () {
        $scope.upload($scope.files);
    });

    $scope.upload = function (files) {
        if (files && files.length) {
            for (var i = 0; i < files.length; i++) {
                var file = files[i];
                Upload.upload({
                    url: '/upload/question',
                    file: file
                }).progress(function (evt) {
                    var progressPercentage = parseInt(100.0 * evt.loaded / evt.total);
                    $scope.determinateValue = progressPercentage;
                }).success(function (data, status, headers, config) {
                    console.log("Data ");
                    console.log(data);
                    $rootScope.currentUploadMessageId = data._id;
                    $rootScope.fileUploaded = true;
                });
            }
        }
    };
}]);

app.controller('messageUpload', ['$scope', '$rootScope', 'Upload', function ($scope, $rootScope, Upload) {
    $scope.mode = 'query';
    $scope.determinateValue = 0;
    $scope.$watch('files', function () {
        $scope.upload($scope.files);
    });

    $scope.upload = function (files) {
        if (files && files.length) {
            for (var i = 0; i < files.length; i++) {
                var file = files[i];
                Upload.upload({
                    url: '/upload/message',
                    file: file
                }).progress(function (evt) {
                    var progressPercentage = parseInt(100.0 * evt.loaded / evt.total);
                    $scope.determinateValue = progressPercentage;
                }).success(function (data, status, headers, config) {
                    console.log("Data ");
                    console.log(data);
                    $rootScope.currentUploadMessageId = data._id;
                    $rootScope.fileUploaded = true;
                });
            }
        }
    };
}]);
