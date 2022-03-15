package com.shieldblaze.expressgateway.restapi.response;

import com.google.gson.JsonElement;
import com.shieldblaze.expressgateway.common.Gson;
import com.shieldblaze.expressgateway.restapi.response.builder.APIResponse;
import com.shieldblaze.expressgateway.restapi.response.builder.ErrorMessage;
import com.shieldblaze.expressgateway.restapi.response.builder.Message;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaderValues;
import io.netty.handler.codec.http.HttpResponseStatus;
import org.springframework.http.HttpStatus;
import org.springframework.http.ResponseEntity;
import org.springframework.util.LinkedMultiValueMap;
import org.springframework.util.MultiValueMap;

/**
 * {@link FastBuilder} is used for building {@link ResponseEntity} quickly.
 */
public final class FastBuilder {

    private static final MultiValueMap<String, String> MULTI_VALUE_MAP = new LinkedMultiValueMap<>();
    static {
        MULTI_VALUE_MAP.set(HttpHeaderNames.CONTENT_TYPE.toString(), HttpHeaderValues.APPLICATION_JSON.toString());
    }

    /**
     * Build error {@link ResponseEntity} with {@link APIResponse.APIResponseBuilder#isSuccess(boolean)} set to {@code false}
     * and {@link Error} built using {@link ErrorBase}
     *
     * @param errorBase          {@link ErrorBase} to build {@link Error}
     * @param httpResponseStatus {@link HttpResponseStatus} for HTTP Response Code
     * @return {@link ResponseEntity} containing error body, response code and headers.
     */
    public static ResponseEntity<String> error(ErrorBase errorBase, HttpResponseStatus httpResponseStatus) {
        // Build Error
        ErrorMessage error = ErrorMessage.newBuilder()
                .withErrorBase(errorBase)
                .build();

        // Build Response
        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(false)
                .withError(error)
                .build();

        // Return the Response
        return response(apiResponse.getResponse(), httpResponseStatus);
    }

    /**
     * Build error {@link ResponseEntity} with {@link APIResponse.APIResponseBuilder#isSuccess(boolean)} set to {@code false}
     * and {@link Error} built using {@link ErrorBase}
     *
     * @param errorBase          {@link ErrorBase} to build {@link Error}
     * @param httpResponseStatus {@link HttpResponseStatus} for HTTP Response Code
     * @return {@link ResponseEntity} containing error body, response code and headers.
     */
    public static ResponseEntity<String> error(ErrorBase errorBase, String message, HttpResponseStatus httpResponseStatus) {
        // Build Error
        ErrorMessage error = ErrorMessage.newBuilder()
                .withErrorBase(errorBase)
                .build();

        // Build Response
        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(false)
                .withError(error)
                .withMessage(Message.newBuilder().withHeader("ErrorMessage").withMessage(message).build())
                .build();

        // Return the Response
        return response(apiResponse.getResponse(), httpResponseStatus);
    }

    /**
     * Build error {@link ResponseEntity} with {@link APIResponse.APIResponseBuilder#isSuccess(boolean)} set to {@code false}
     * , {@link Error} built using {@link ErrorBase} and {@link Message}
     *
     * @param errorBase          {@link ErrorBase} to build {@link Error} in {@link APIResponse.APIResponseBuilder#withError(ErrorMessage)}
     * @param message            {@link Message} in {@link APIResponse.APIResponseBuilder#withMessage(Message)}
     * @param httpResponseStatus {@link HttpResponseStatus} for HTTP Response Code
     * @return {@link ResponseEntity} containing error body, response code and headers.
     */
    public static ResponseEntity<String> error(ErrorBase errorBase, Message message, HttpResponseStatus httpResponseStatus) {
        // Build Error
        ErrorMessage error = ErrorMessage.newBuilder()
                .withErrorBase(errorBase)
                .build();

        // Build Response
        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(false)
                .withError(error)
                .withMessage(message)
                .build();

        // Return the Response
        return response(apiResponse.getResponse(), httpResponseStatus);
    }

    /**
     * Build {@link ResponseEntity} using {@link JsonElement} and {@link HttpResponseStatus}
     *
     * @param jsonElement        {@link JsonElement} containing body
     * @param httpResponseStatus {@link HttpResponseStatus} for HTTP Response Code
     * @return {@link ResponseEntity} containing body, response code and headers.
     */
    public static ResponseEntity<String> response(JsonElement jsonElement, HttpResponseStatus httpResponseStatus) {
        return new ResponseEntity<>(Gson.INSTANCE.toJson(jsonElement), MULTI_VALUE_MAP, HttpStatus.valueOf(httpResponseStatus.code()));
    }
}
