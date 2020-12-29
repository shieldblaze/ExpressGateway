package com.shieldblaze.expressgateway.restapi.response.builder;

public enum ErrorBase {

    // API Errors
    INTERNAL_SERVER_ERROR(100, "Internal Server Error"),


    LOADBALANCER_NOT_FOUND(200, "LoadBalancer not found")

    ;

    private final int ErrorCode;
    private final String ErrorMessage;

    ErrorBase(int ErrorCode, String ErrorMessage) {
        this.ErrorCode = ErrorCode;
        this.ErrorMessage = ErrorMessage;
    }

    public int getErrorCode() {
        return ErrorCode;
    }

    public String getErrorMessage() {
        return ErrorMessage;
    }
}