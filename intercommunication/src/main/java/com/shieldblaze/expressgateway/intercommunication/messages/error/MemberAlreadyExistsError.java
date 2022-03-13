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
package com.shieldblaze.expressgateway.intercommunication.messages.error;

import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.intercommunication.Errors;
import com.shieldblaze.expressgateway.intercommunication.messages.ResponseMessage;
import io.netty.buffer.ByteBuf;

/**
 * See {@link Errors#MEMBER_ALREADY_EXISTS}
 */
public final class MemberAlreadyExistsError implements ResponseMessage {

    private final ByteBuf id;

    @NonNull
    public MemberAlreadyExistsError(ByteBuf id) {
        this.id = id;
    }

    @Override
    public ByteBuf id() {
        return id;
    }
}
