var app = angular.module('TelephoneBoothApp', ['ngMaterial', 'ngFileUpload']);

app.controller('PhoneBothCtrl', ['$scope', '$mdDialog', '$http', function($scope, $mdDialog, $http){
    $scope.addNewQuestion = function(ev) {
        $mdDialog.show({
			controller: NewQuestionController,
			templateUrl: '/modals/new_question',
			targetEvent: ev,
			onComplete: afterShowAnimation
		})
		.then(function(answer) {
			$scope.alert = 'You said the information was "' + answer + '".';
			$http.get('/questions').success(function(response) {
				$scope.questions = response;
			});
		}, function() {
			$scope.alert = 'You cancelled the dialog.';
		});

		function afterShowAnimation(scope, element, options) {
           // post-show code here: DOM element focus, etc.
		   $('#newMap').focus();
        }
    };

    function NewQuestionController($scope, $mdDialog, $http) {
        $scope.answer = '';
        $scope.hide = function() {
            $mdDialog.hide();
        };

	$scope.cancel = function() {
		console.log('Canceled');
		$mdDialog.cancel();
	};

	$scope.answer = function(type) {
		console.log('New Map! ' + $scope.map.title);
		if (type == 'New Map') {
			var postData = JSON.stringify({'title': $scope.map.title});
			console.log(postData);
			$http({
				method: 'POST',
				url: '/map',
				data: postData,
				contentType: 'application/json', // content type sent to server
				dataType: 'json', //Expected data format from server
				processdata: true, //True or False
				crossDomain: true,
			}).success(function(response) {
				console.log('Whee! ' + response.codeStatus);
				$mdDialog.hide();
			}).error(function(response) {
				console.log("error"); // Getting Error Response in Callback
				$scope.codeStatus = response || "Request failed";
				console.log($scope.codeStatus);
			});
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
              $scope.$emit('loadEvent');
              $rootScope.
          });
        }
      }
    };
}]);
