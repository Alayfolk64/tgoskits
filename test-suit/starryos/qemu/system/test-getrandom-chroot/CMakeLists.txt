cmake_minimum_required(VERSION 3.20)
project(test-getrandom-chroot C)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)
set(CMAKE_C_EXTENSIONS OFF)

add_executable(test-getrandom-chroot src/main.c)
target_compile_options(test-getrandom-chroot PRIVATE -Wall -Wextra -Werror)
install(TARGETS test-getrandom-chroot RUNTIME DESTINATION usr/bin/starry-test-suit)
