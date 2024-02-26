package com.shieldblaze.expressgateway.restapi.response.builder;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;

import java.util.Collections;
import java.util.List;

/**
 * <p> Response and Response Builder </p>
 */
public final class APIResponse {

    private final JsonObject finalResponse;

    private APIResponse(APIResponseBuilder APIResponseBuilder) {
        finalResponse = APIResponseBuilder.finalResponse;
    }

    /**
     * Create a new ResponseBuilder
     *
     * @return ResponseBuilder
     */
    public static APIResponseBuilder newBuilder() {
        return new APIResponseBuilder();
    }

    /**
     * Get Built response
     *
     * @return JsonObject of response
     */
    public JsonObject getResponse() {
        return finalResponse;
    }

    public static final class APIResponseBuilder {

        private final JsonObject response = new JsonObject();
        private final JsonObject finalResponse = new JsonObject();

        private List<ErrorMessage> errorList;
        private ErrorMessage errorMessage;
        private List<Message> MessagesList;
        private Message Message;
        private List<Result> ResultList;
        private boolean Success;

        public APIResponseBuilder isSuccess(boolean isSuccess) {
            Success = isSuccess;
            return this;
        }

        public APIResponseBuilder withErrors(List<ErrorMessage> errorList) {
            this.errorList = errorList;
            return this;
        }

        public APIResponseBuilder withError(ErrorMessage errorMessage) {
            this.errorMessage = errorMessage;
            return this;
        }

        public APIResponseBuilder withMessages(List<Message> messageList) {
            MessagesList = messageList;
            return this;
        }

        public APIResponseBuilder withMessage(Message message) {
            Message = message;
            return this;
        }

        public APIResponseBuilder withResult(Result result) {
            ResultList = Collections.singletonList(result);
            return this;
        }


        public APIResponseBuilder withResults(List<Result> resultList) {
            ResultList = resultList;
            return this;
        }

        /**
         * Build the final Json Response
         *
         * @return Response Class
         */
        public APIResponse build() {

            /*
             * Build Success response
             */
            {
                response.addProperty("Success", Success);
            }

            /*
             * Build Error response
             */
            {
                if (errorList != null && !errorList.isEmpty()) {
                    JsonArray jsonArray = new JsonArray();
                    for (ErrorMessage error : errorList) {
                        JsonObject errorBody = new JsonObject();
                        errorBody.addProperty("Code", error.getErrorCode());
                        errorBody.addProperty("Message", error.getErrorMessage());
                        jsonArray.add(errorBody);
                    }
                    response.add("Errors", jsonArray);
                } else if (errorMessage != null) {
                    JsonObject errorBody = new JsonObject();
                    errorBody.addProperty("Code", errorMessage.getErrorCode());
                    errorBody.addProperty("Message", errorMessage.getErrorMessage());
                    JsonArray jsonArray = new JsonArray();
                    jsonArray.add(errorBody);
                    response.add("Errors", jsonArray);
                } else {
                    response.add("Errors", new JsonArray());
                }
            }

            /*
             * Build Message response
             */
            {
                if (MessagesList != null && !MessagesList.isEmpty()) {
                    JsonArray jsonArray = new JsonArray();
                    for (Message message : MessagesList) {
                        JsonObject errorBody = new JsonObject();
                        errorBody.addProperty(message.getHeader(), message.getMessage());
                        jsonArray.add(errorBody);
                    }
                    response.add("Messages", jsonArray);
                } else if (Message != null) {
                    JsonObject errorBody = new JsonObject();
                    errorBody.addProperty(Message.getHeader(), Message.getMessage());
                    JsonArray jsonArray = new JsonArray();
                    jsonArray.add(errorBody);
                    response.add("Messages", jsonArray);
                } else {
                    response.add("Messages", new JsonArray());
                }
            }

            /*
             * Build Result response
             */
            {
                if (ResultList != null && !ResultList.isEmpty()) {
                    JsonObject resultBody = new JsonObject();
                    for (Result result : ResultList) {
                        if (result.getMessage() instanceof JsonElement) {
                            resultBody.add(result.getHeader(), (JsonElement) result.getMessage());
                        } else if (result.getMessage() instanceof String) {
                            resultBody.addProperty(result.getHeader(), (String) result.getMessage());
                        } else if (result.getMessage() instanceof Number) {
                            resultBody.addProperty(result.getHeader(), (Number) result.getMessage());
                        } else if (result.getMessage() instanceof Boolean) {
                            resultBody.addProperty(result.getHeader(), (Boolean) result.getMessage());
                        } else if (result.getMessage() instanceof Character) {
                            resultBody.addProperty(result.getHeader(), (Character) result.getMessage());
                        }
                    }
                    response.add("Result", resultBody);
                } else {
                    response.add("Result", new JsonObject());
                }
            }

            finalResponse.add("Success", response.get("Success"));
            finalResponse.add("Errors", response.get("Errors"));
            finalResponse.add("Messages", response.get("Messages"));
            finalResponse.add("Result", response.get("Result"));

            return new APIResponse(this);
        }
    }
}
