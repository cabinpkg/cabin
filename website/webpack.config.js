module.exports = (env, argv) => ({
  entry: {
    index: './main.js',
  },

  output: {
    path: `${__dirname}/dist`,
    filename: 'main.js',
  },

  plugins: [
    new (require('copy-webpack-plugin'))({
      patterns: [
        { from: 'assets/', to: './' },
        { from: 'index.html', to: './' },
      ],
    }),
  ],

  module: {
    rules: [
      {
        test: /\.elm$/,
        include: [/src/],
        loader: "elm-webpack-loader",
      },
    ],
    noParse: [/\.elm$/],
  },
});
