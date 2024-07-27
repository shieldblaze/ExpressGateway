package com.shieldblaze.expressgateway.restapi.response.builder;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;

import java.util.Collections;
import java.util.List;

import static java.util.Collections.unmodifiableList;

/**
 * <p> Response and Response Builder </p>
 */
public final class APIResponse {

    private static final JsonObject EMPTY_JSON_OBJECT = new JsonObject();
    private static final JsonArray EMPTY_JSON_ARRAY = new JsonArray();

    private final JsonObject finalResponse;

    private APIResponse(APIResponseBuilder apiResponseBuilder) {
        finalResponse = apiResponseBuilder.finalResponse;
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
    public JsonObject response() {
        return finalResponse;
    }

    public static final class APIResponseBuilder {

        private final JsonObject response = new JsonObject();
        private final JsonObject finalResponse = new JsonObject();

        private List<ErrorMessage> errorList;
        private ErrorMessage errorMessage;
        private List<Message> messageList;
        private Message Message;
        private List<Result> resultList;
        private boolean success;

        public APIResponseBuilder isSuccess(boolean isSuccess) {
            success = isSuccess;
            return this;
        }

        public APIResponseBuilder withErrors(List<ErrorMessage> errorList) {
            this.errorList = unmodifiableList(errorList);
            return this;
        }

        public APIResponseBuilder withError(ErrorMessage errorMessage) {
            this.errorMessage = errorMessage;
            return this;
        }

        public APIResponseBuilder withMessages(List<Message> messageList) {
            this.messageList = unmodifiableList(messageList);
            return this;
        }

        public APIResponseBuilder withMessage(Message message) {
            Message = message;
            return this;
        }

        public APIResponseBuilder withResult(Result result) {
            resultList = Collections.singletonList(result);
            return this;
        }


        public APIResponseBuilder withResults(List<Result> resultList) {
            this.resultList = unmodifiableList(resultList);
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
                response.addProperty("Success", success);
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
                    response.add("Errors", EMPTY_JSON_ARRAY);
                }
            }

            /*
             * Build Message response
             */
            {
                if (messageList != null && !messageList.isEmpty()) {
                    JsonArray jsonArray = new JsonArray();
                    for (Message message : messageList) {
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
                    response.add("Messages", EMPTY_JSON_ARRAY);
                }
            }

            /*
             * Build Result response
             */
            {
                if (resultList != null && !resultList.isEmpty()) {
                    JsonObject resultBody = new JsonObject();
                    for (Result result : resultList) {
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
                    response.add("Result", EMPTY_JSON_OBJECT);
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
