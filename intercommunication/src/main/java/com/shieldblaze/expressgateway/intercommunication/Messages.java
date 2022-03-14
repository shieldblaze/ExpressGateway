/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
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
package com.shieldblaze.expressgateway.intercommunication;

import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;

final class Messages {

    private static final ByteBuf DELIMITER = Unpooled.unreleasableBuffer(Unpooled.buffer(4, 4).writeInt(0xff123abc)).asReadOnly();
    private static final ByteBuf MAGIC = Unpooled.unreleasableBuffer(Unpooled.buffer(4, 4).writeInt(0xabc123ff)).asReadOnly();
    private static final ByteBuf JOIN_REQUEST = Unpooled.unreleasableBuffer(Unpooled.buffer(2, 2).writeShort(0xa)).asReadOnly();
    private static final ByteBuf LEAVE_REQUEST = Unpooled.unreleasableBuffer(Unpooled.buffer(2, 2).writeShort(0xb)).asReadOnly();
    private static final ByteBuf UPSET_DATA_REQUEST = Unpooled.unreleasableBuffer(Unpooled.buffer(2, 2).writeShort(0xc)).asReadOnly();
    private static final ByteBuf DELETE_DATA_REQUEST = Unpooled.unreleasableBuffer(Unpooled.buffer(2, 2).writeShort(0xd)).asReadOnly();
    private static final ByteBuf SIMPLE_MESSAGE_REQUEST = Unpooled.unreleasableBuffer(Unpooled.buffer(2, 2).writeShort(0xe)).asReadOnly();
    private static final ByteBuf JOIN_RESPONSE = Unpooled.unreleasableBuffer(Unpooled.buffer(2, 2).writeShort(0xf)).asReadOnly();
    private static final ByteBuf LEAVE_RESPONSE = Unpooled.unreleasableBuffer(Unpooled.buffer(2, 2).writeShort(0xaa)).asReadOnly();
    private static final ByteBuf UPSET_DATA_RESPONSE = Unpooled.unreleasableBuffer(Unpooled.buffer(2, 2).writeShort(0xab)).asReadOnly();
    private static final ByteBuf DELETE_DATA_RESPONSE = Unpooled.unreleasableBuffer(Unpooled.buffer(2, 2).writeShort(0xac)).asReadOnly();
    private static final ByteBuf SIMPLE_MESSAGE_RESPONSE = Unpooled.unreleasableBuffer(Unpooled.buffer(2, 2).writeShort(0xad)).asReadOnly();

    static ByteBuf DELIMITER() {
        return DELIMITER.duplicate();
    }

    /**
     * Magic number header
     */
    static ByteBuf MAGIC() {
        return MAGIC.duplicate();
    }

    /**
     * Member Join Request
     */
    static ByteBuf JOIN_REQUEST(){
        return JOIN_REQUEST.duplicate();
    }

    /**
     * Member Leave Request
     */
    static ByteBuf LEAVE_REQUEST() {
        return LEAVE_REQUEST.duplicate();
    }

    /**
     * Delete data Request
     */
    static ByteBuf UPSET_DATA_REQUEST() {
        return UPSET_DATA_REQUEST.duplicate();
    }

    /**
     * Delete data Request
     */
    static ByteBuf DELETE_DATA_REQUEST() {
        return DELETE_DATA_REQUEST.duplicate();
    }

    /**
     * Simple Message Request
     */
    static ByteBuf SIMPLE_MESSAGE_REQUEST() {
        return SIMPLE_MESSAGE_REQUEST.duplicate();
    }

    /**
     * Member Join Response
     */
    static ByteBuf JOIN_RESPONSE() {
        return JOIN_RESPONSE.duplicate();
    }

    /**
     * Member Leave Response
     */
    static ByteBuf LEAVE_RESPONSE() {
        return LEAVE_RESPONSE.duplicate();
    }

    /**
     * Upsert data Response
     */
    static ByteBuf UPSET_DATA_RESPONSE() {
        return UPSET_DATA_RESPONSE.duplicate();
    }

    /**
     * Delete data Response
     */
    static ByteBuf DELETE_DATA_RESPONSE() {
        return DELETE_DATA_RESPONSE.duplicate();
    }

    /**
     * Simple Message Response
     */
    static ByteBuf SIMPLE_MESSAGE_RESPONSE() {
        return SIMPLE_MESSAGE_RESPONSE.duplicate();
    }
}
