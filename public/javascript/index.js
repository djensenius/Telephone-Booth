var app = angular.module('TelephoneBoothApp', ['ngMaterial', 'ngFileUpload', 'ngAudio']);

app.controller('PhoneBothCtrl', ['$scope', '$mdDialog', '$http', '$rootScope', 'ngAudio', 'socket', function($scope, $mdDialog, $http, $rootScope, ngAudio, socket){

  $scope.questions = [];
  $scope.pendings = [];
  $scope.approved = [];
  $scope.rejected = [];
  $scope.sound = [];
  $scope.loading = {};
  $scope.downloaded = {};
  $scope.questionIndex = {};
  $scope.questionPlays = 0;
  $scope.messagePlays = 0;

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
      $scope.questions = response;
      $scope.questionPlays = 0;
      for (var i = 0; i < response.length; i++) {
        if (response[i].playCount) {
          $scope.questionPlays = $scope.questionPlays + response[i].playCount;          
          var id = response[i]._id;
          $scope.questionIndex[id] = {description: response[i].description, voice: response[i].voice};
        }
      }
    });
  }
  loadQuestions();

  function loadPending() {
    $http.get('/pending').success(function(response) {
      $scope.pendings = response;
    });
  }
  loadPending();

  function loadApproved() {
    $http.get('/approved').success(function(response) {
      $scope.approved = response.reverse();
      $scope.messagePlays = 0;
      for (var i = 0; i < response.length; i++) {
        if (response[i].playCount) {
          $scope.messagePlays = $scope.messagePlays + response[i].playCount;
        }
      }
    });
  }
  loadApproved()

  function loadRejected() {
    $http.get('/rejected').success(function(response) {
      $scope.rejected = response.reverse();
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
    //console.log("Status is: ", data);
    $scope.lastSeen = data.ping;
    $scope.listeningMessage = data.listeningMessage;
    $scope.listeningQuestion = data.listeningQuestion;
    $scope.recording = data.recording;
    $scope.hook = data.hook;
  });

  socket.on('updateQuestion', function(data) {
    loadQuestions();
  });

  socket.on('updateMessages', function(data) {
    reloadMessages();
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

  $scope.loadSound = function(sound_id, fileName, type) {
    console.log(fileName + " " + sound_id);
    $scope.loading[fileName] = true;
    var extension = fileName.split('.').pop();
    if (type == "question") {
      $scope.sound[sound_id] = ngAudio.load("/download/question/" + sound_id + "." + extension);
    } else if (type == "message") {
      $scope.sound[sound_id] = ngAudio.load("/download/message/" + sound_id + "." + extension);
    }
    $scope.downloaded[sound_id] = true;
    console.log("Set download to true...");
    //$scope.sound[sound_id] = ngAudio.load("/download/message/" + sound_id + "." + extension);
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
