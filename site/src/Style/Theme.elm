module Style.Theme exposing (select)

import Css exposing (backgroundColor, borderColor, color, hex)
import Css.Colors exposing (black, white)
import Model exposing (Theme)


select : Bool -> Theme
select isDarkTheme =
    if isDarkTheme then
        dark

    else
        light


dark : Theme
dark =
    Theme
        (color white)
        (backgroundColor (hex "1E1E1E"))
        (borderColor white)


light : Theme
light =
    Theme
        (color black)
        (backgroundColor white)
        (borderColor black)
