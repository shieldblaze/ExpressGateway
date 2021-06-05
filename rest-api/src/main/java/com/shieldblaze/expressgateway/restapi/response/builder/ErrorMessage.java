package com.shieldblaze.expressgateway.restapi.response.builder;

import com.shieldblaze.expressgateway.restapi.response.ErrorBase;

public class ErrorMessage {

    private final int ErrorCode;
    private final String ErrorMessage;

    private ErrorMessage(ErrorBuilder errorBuilder) {
        ErrorCode = errorBuilder.ErrorCode;
        ErrorMessage = errorBuilder.ErrorMessage;
    }

    /**
     * Create a new ErrorBuilder
     *
     * @return ErrorBuilder
     */
    public static ErrorBuilder newBuilder() {
        return new ErrorBuilder();
    }

    public int getErrorCode() {
        return ErrorCode;
    }

    public String getErrorMessage() {
        return ErrorMessage;
    }

    public static final class ErrorBuilder {
        private int ErrorCode;
        private String ErrorMessage;
        private ErrorBase errorBase = null;

        public ErrorBuilder withErrorBase(ErrorBase errorBase) {
            this.errorBase = errorBase;
            return this;
        }

        public com.shieldblaze.expressgateway.restapi.response.builder.ErrorMessage build() {

            if (errorBase != null) {
                this.ErrorCode = errorBase.code();
                this.ErrorMessage = errorBase.message();
            }

            return new ErrorMessage(this);
        }
    }
}
