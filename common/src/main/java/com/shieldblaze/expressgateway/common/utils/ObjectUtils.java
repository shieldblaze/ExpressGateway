package com.shieldblaze.expressgateway.common.utils;

public final class ObjectUtils {

    private ObjectUtils() {
        // Prevent outside initialization
    }

    /**
     * Check if the object is not null and throw a {@link NullPointerException} if it is.
     * </p>
     * Exception message will be "Object cannot be 'null'"
     *
     * @param obj   The object to check
     * @param clazz The class of the object
     * @param <T>   The type of the object
     * @return The object if it is not null
     */
    public static <T> T nonNull(T obj, Class<?> clazz) {
        return nonNull(obj, clazz.getSimpleName());
    }

    /**
     * Check if the object is not null and throw a {@link NullPointerException} if it is.
     *
     * @param obj     The object to check
     * @param message The message to throw with the exception
     * @param <T>     The type of the object
     * @return The object if it is not null
     */
    public static <T> T nonNull(T obj, String message) {
        if (obj == null) {
            throw new NullPointerException(message);
        }
        return obj;
    }

    /**
     * Check if the object is not null and throw a {@link NullPointerException} if it is.
     * </p>
     * Exception message will be "Object cannot be 'null'"
     *
     * @param obj    The object to check
     * @param object The name of the object
     * @param <T>    The type of the object
     * @return The object if it is not null
     * @throws NullPointerException If the object is null
     */
    public static <T> T nonNullObject(T obj, String object) {
        if (obj == null) {
            throw new NullPointerException(object + " cannot be 'null'");
        }
        return obj;
    }
}
