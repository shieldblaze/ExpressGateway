package com.shieldblaze.expressgateway.restapi.response.builder;

public class Error {

    private final int ErrorCode;
    private final String ErrorMessage;

    private Error(ErrorBuilder errorBuilder) {
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

        public Error build() {

            if (errorBase != null) {
                this.ErrorCode = errorBase.getErrorCode();
                this.ErrorMessage = errorBase.getErrorMessage();
            }

            return new Error(this);
        }
    }

}
