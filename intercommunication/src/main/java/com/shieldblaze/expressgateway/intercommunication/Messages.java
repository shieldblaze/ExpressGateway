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

final class Messages {

    /**
     * Delimiter
     */
    static final int DELIMITER = 0xff123abc;

    /**
     * Magic number header
     */
    static final int MAGIC = 0xabc123ff;

    /**
     * Member Join Request
     */
    static final short JOIN_REQUEST = 0xa;

    /**
     * Member Leave Request
     */
    static final short LEAVE_REQUEST = 0xb;

    /**
     * Upsert data Request
     */
    static final short UPSET_DATA_REQUEST = 0xc;

    /**
     * Delete data Request
     */
    static final short DELETE_DATA_REQUEST = 0xd;

    /**
     * Simple Message Request
     */
    static final short SIMPLE_MESSAGE_REQUEST = 0xe;

    /**
     * Member Join Response
     */
    static final short JOIN_RESPONSE = 0xf;

    /**
     * Member Leave Response
     */
    static final short LEAVE_RESPONSE = 0xaa;

    /**
     * Upsert data Response
     */
    static final short UPSET_DATA_RESPONSE = 0xab;

    /**
     * Delete data Response
     */
    static final short DELETE_DATA_RESPONSE = 0xac;

    /**
     * Simple Message Response
     */
    static final short SIMPLE_MESSAGE_RESPONSE = 0xad;
}
