/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.restapi.response.builder;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.shieldblaze.expressgateway.common.GSON;

import java.util.Collections;
import java.util.List;

/**
 * <p> Response and Response Builder </p>
 */
public final class APIResponse {

    private final JsonObject finalResponse;

    private APIResponse(APIResponseBuilder APIResponseBuilder) {
        this.finalResponse = APIResponseBuilder.finalResponse;
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

    public String response() {
        return GSON.INSTANCE.toJson(getResponse());
    }

    public static final class APIResponseBuilder {

        private final JsonObject response = new JsonObject();
        private final JsonObject finalResponse = new JsonObject();

        private List<Error> ErrorList;
        private Error Error;
        private List<Message> MessagesList;
        private Message Message;
        private List<Result> ResultList;
        private boolean Success;

        public APIResponseBuilder isSuccess(boolean isSuccess) {
            this.Success = isSuccess;
            return this;
        }

        public APIResponseBuilder withErrors(List<Error> errorList) {
            this.ErrorList = errorList;
            return this;
        }

        public APIResponseBuilder withError(Error error) {
            this.Error = error;
            return this;
        }

        public APIResponseBuilder withMessages(List<Message> messageList) {
            this.MessagesList = messageList;
            return this;
        }

        public APIResponseBuilder withMessage(Message message) {
            this.Message = message;
            return this;
        }

        public APIResponseBuilder withResult(Result result) {
            this.ResultList = Collections.singletonList(result);
            return this;
        }


        public APIResponseBuilder withResults(List<Result> resultList) {
            this.ResultList = resultList;
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
                if (ErrorList != null && ErrorList.size() != 0) {
                    JsonArray jsonArray = new JsonArray();
                    for (Error error : ErrorList) {
                        JsonObject errorBody = new JsonObject();
                        errorBody.addProperty("Code", error.getErrorCode());
                        errorBody.addProperty("Message", error.getErrorMessage());
                        jsonArray.add(errorBody);
                    }
                    response.add("Errors", jsonArray);
                } else if (Error != null) {
                    JsonObject errorBody = new JsonObject();
                    errorBody.addProperty("Code", Error.getErrorCode());
                    errorBody.addProperty("Message", Error.getErrorMessage());
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
                if (MessagesList != null && MessagesList.size() > 0) {
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
                if (ResultList != null && ResultList.size() > 0) {
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
