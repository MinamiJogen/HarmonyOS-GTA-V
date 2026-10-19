#include "../../harmony/entry/src/main/cpp/game_resource_import_core.h"
#include <atomic>
#include <fcntl.h>
#include <iostream>
#include <stdexcept>
#include <string>
#include <unistd.h>

int main(int argc, char** argv) {
    if (argc < 3) return 64;
    std::string operation = argv[1];
    game_resources::Result result;
    if (operation == "check") result = game_resources::Check(argv[2]);
    else {
        if (argc < 4) return 64;
        int fd = open(argv[2], O_RDONLY);
        if (fd < 0) return 66;
        std::atomic<bool> cancelled{false};
        uint64_t cancelAfter = 0;
        bool throwCommit = false;
        game_resources::Limits limits;
        for (int i = 4; i < argc; ++i) {
            std::string option = argv[i];
            if (option == "cancel-before") cancelled = true;
            else if (option == "throw-commit") throwCommit = true;
            else if (option.compare(0, 6, "total=") == 0) limits.maxTotalBytes = std::stoull(option.substr(6));
            else if (option.compare(0, 5, "file=") == 0) limits.maxFileBytes = std::stoull(option.substr(5));
            else if (option.compare(0, 7, "cancel=") == 0) cancelAfter = std::stoull(option.substr(7));
        }
        result = game_resources::ImportZip(fd, argv[3], cancelled,
            [&](const game_resources::Progress& progress) {
                if (cancelAfter && progress.completedBytes >= cancelAfter) cancelled = true;
                if (throwCommit && progress.phase == "committing") throw std::runtime_error("injected commit failure");
            }, limits);
        close(fd);
    }
    std::cout << game_resources::ToJson(result) << '\n';
    return result.ready ? 0 : 2;
}
