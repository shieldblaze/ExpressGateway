package com.shieldblaze.expressgateway.restapi.response.builder;

public class Message {
    private final String Header;
    private final String Message;

    private Message(MessageBuilder messageBuilder) {
        Header = messageBuilder.Header;
        Message = messageBuilder.Message;
    }

    public static MessageBuilder newBuilder() {
        return new MessageBuilder();
    }

    public String getHeader() {
        return Header;
    }

    public String getMessage() {
        return Message;
    }

    public static final class MessageBuilder {
        private String Header;
        private String Message;

        public MessageBuilder withHeader(String Header) {
            this.Header = Header;
            return this;
        }

        public MessageBuilder withMessage(String Message) {
            this.Message = Message;
            return this;
        }

        public Message build() {
            return new Message(this);
        }
    }
}
