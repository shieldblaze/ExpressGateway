package com.shieldblaze.expressgateway.restapi.response.builder;

public class Result {
    private final String Header;
    private final Object Message;

    private Result(ResultBuilder resultBuilder) {
        Header = resultBuilder.Header;
        Message = resultBuilder.Message;
    }

    /**
     * Create a new ResultBuilder
     *
     * @return ResultBuilder
     */
    public static ResultBuilder newBuilder() {
        return new ResultBuilder();
    }

    public String getHeader() {
        return Header;
    }

    public Object getMessage() {
        return Message;
    }

    public static final class ResultBuilder {
        private String Header;
        private Object Message;

        public ResultBuilder withHeader(String Header) {
            this.Header = Header;
            return this;
        }

        public ResultBuilder withMessage(Object Message) {
            this.Message = Message;
            return this;
        }

        public Result build() {
            return new Result(this);
        }
    }
}
