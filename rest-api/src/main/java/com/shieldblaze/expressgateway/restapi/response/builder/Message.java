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
